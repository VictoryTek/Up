use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::runner::{BackendEvent, CommandRunner, PrivilegedShell};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// A cancel handle returned by [`UpdateOrchestrator::run_all`].
///
/// Clone freely; [`cancel`][CancelHandle::cancel] is safe to call from any
/// thread including the GTK main thread.
#[derive(Clone)]
pub struct CancelHandle {
    cancelled: Arc<AtomicBool>,
    shell_slot: Arc<Mutex<Option<Arc<tokio::sync::Mutex<PrivilegedShell>>>>>,
}

impl CancelHandle {
    /// Signal the orchestrator to stop after the current backend finishes.
    ///
    /// If a privileged shell is active its stdin is closed, causing it to exit
    /// after the current command completes.  Safe to call multiple times.
    pub fn cancel(&self) {
        if self.cancelled.swap(true, Ordering::SeqCst) {
            return; // already cancelled
        }
        let slot = self.shell_slot.clone();
        drop(crate::runtime::runtime().spawn(async move {
            let maybe_shell = {
                let mut guard = slot.lock().expect("shell_slot mutex poisoned");
                guard.take()
            };
            if let Some(shell_arc) = maybe_shell {
                shell_arc.lock().await.close().await;
            }
        }));
    }

    /// Returns `true` if [`cancel`][CancelHandle::cancel] has been called.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// Events emitted by the orchestrator to the UI layer during an update run.
pub enum OrchestratorEvent {
    /// Root authentication is required and has been initiated.
    AuthStarted,
    /// Authentication succeeded (or root was not required) — backends are starting.
    AuthSucceeded,
    /// Authentication failed; contains the error message.
    AuthFailed(String),
    /// The named backend has started its update operation.
    BackendStarted(BackendKind),
    /// A single line of log output produced by the named backend.
    BackendLog(BackendKind, String),
    /// The named backend has finished; carries its result.
    BackendFinished(BackendKind, UpdateResult),
    /// All backends have finished; no more events will be sent.
    AllFinished,
}

/// One entry in the orchestrator's backend list.
/// `None` means run a full update; `Some(ids)` means update only those items.
pub type BackendSelection = (Arc<dyn Backend>, Option<Vec<String>>);

/// Drives the update sequence for a set of backends, sending progress events
/// to the UI via an [`async_channel`].  Does not hold any GTK types.
pub struct UpdateOrchestrator {
    backends: Vec<BackendSelection>,
}

impl UpdateOrchestrator {
    pub fn new(backends: Vec<BackendSelection>) -> Self {
        Self { backends }
    }

    /// Spawn the update work on a background OS thread and stream
    /// [`OrchestratorEvent`] messages to `tx` in arrival order.
    ///
    /// The caller (GTK main thread) should receive from the matching receiver
    /// and update the UI based on the events.
    pub fn run_all(&self, tx: async_channel::Sender<OrchestratorEvent>) -> CancelHandle {
        let backends = self.backends.clone();

        let cancelled = Arc::new(AtomicBool::new(false));
        let shell_slot: Arc<Mutex<Option<Arc<tokio::sync::Mutex<PrivilegedShell>>>>> =
            Arc::new(Mutex::new(None));
        let handle = CancelHandle {
            cancelled: cancelled.clone(),
            shell_slot: shell_slot.clone(),
        };

        spawn_background(move || async move {
            let any_needs_root = backends.iter().any(|(b, _)| b.needs_root());

            // --- Authentication phase ---
            let shell: Option<Arc<tokio::sync::Mutex<PrivilegedShell>>> = if any_needs_root {
                let _ = tx.send(OrchestratorEvent::AuthStarted).await;
                match PrivilegedShell::new().await {
                    Ok(s) => {
                        let arc = Arc::new(tokio::sync::Mutex::new(s));
                        // Populate shell_slot so CancelHandle::cancel() can close it.
                        if let Ok(mut guard) = shell_slot.lock() {
                            *guard = Some(arc.clone());
                        }
                        Some(arc)
                    }
                    Err(e) => {
                        let _ = tx.send(OrchestratorEvent::AuthFailed(e)).await;
                        return;
                    }
                }
            } else {
                None
            };

            // Signal that auth is done (or was not required) and backends are starting.
            let _ = tx.send(OrchestratorEvent::AuthSucceeded).await;

            // Internal channel: CommandRunner sends BackendEvent::LogLine here.
            // The forwarding task relays them to the OrchestratorEvent stream in
            // real time while run_update is awaited.
            let (be_tx, be_rx) = async_channel::unbounded::<BackendEvent>();

            let tx_fwd = tx.clone();
            let fwd_handle = tokio::spawn(async move {
                while let Ok(event) = be_rx.recv().await {
                    let BackendEvent::LogLine(k, line) = event;
                    let _ = tx_fwd.send(OrchestratorEvent::BackendLog(k, line)).await;
                }
            });

            // --- Backend iteration ---
            for (backend, selected_items) in &backends {
                let kind = backend.kind();

                // Check for cancellation before starting each backend.
                if cancelled.load(Ordering::SeqCst) {
                    let _ = tx
                        .send(OrchestratorEvent::BackendFinished(
                            kind,
                            UpdateResult::Cancelled,
                        ))
                        .await;
                    continue;
                }

                let _ = tx
                    .send(OrchestratorEvent::BackendStarted(kind.clone()))
                    .await;
                let runner = CommandRunner::new(be_tx.clone(), kind.clone(), shell.clone());

                // Dispatch: use run_selected_update only when the backend supports item
                // selection and a non-empty subset was provided by the UI.
                let result = match selected_items {
                    Some(items) if backend.supports_item_selection() && !items.is_empty() => {
                        backend.run_selected_update(items, &runner).await
                    }
                    _ => backend.run_update(&runner).await,
                };

                // If the user cancelled while this backend was running, override the result.
                let result = if cancelled.load(Ordering::SeqCst) {
                    UpdateResult::Cancelled
                } else {
                    result
                };

                let _ = tx
                    .send(OrchestratorEvent::BackendFinished(kind, result))
                    .await;
            }

            // Close the internal channel so the forwarding task drains and exits.
            drop(be_tx);
            let _ = fwd_handle.await;

            // Clear the shell slot so CancelHandle::cancel() cannot double-close.
            if let Ok(mut guard) = shell_slot.lock() {
                guard.take();
            }

            // Shut down the privileged shell now that all backends are done.
            if let Some(s) = shell {
                s.lock().await.close().await;
            }

            let _ = tx.send(OrchestratorEvent::AllFinished).await;
        });

        handle
    }
}

/// Spawns a background OS thread with a single-threaded Tokio runtime and
/// drives the provided async closure to completion on it.
fn spawn_background<F, Fut>(f: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    drop(crate::runtime::runtime().spawn(f()));
}

/// Drives the cleanup/maintenance sequence for a set of backends, sending
/// progress events to the UI via an [`async_channel`].  Reuses [`OrchestratorEvent`].
pub struct CleanupOrchestrator {
    backends: Vec<Arc<dyn Backend>>,
}

impl CleanupOrchestrator {
    pub fn new(backends: Vec<Arc<dyn Backend>>) -> Self {
        Self { backends }
    }

    /// Spawn the cleanup work on a background OS thread and stream
    /// [`OrchestratorEvent`] messages to `tx` in arrival order.
    pub fn run_all(&self, tx: async_channel::Sender<OrchestratorEvent>) {
        let backends = self.backends.clone();
        spawn_background(move || async move {
            let any_needs_root = backends.iter().any(|b| b.needs_root());

            let shell: Option<Arc<tokio::sync::Mutex<PrivilegedShell>>> = if any_needs_root {
                let _ = tx.send(OrchestratorEvent::AuthStarted).await;
                match PrivilegedShell::new().await {
                    Ok(s) => Some(Arc::new(tokio::sync::Mutex::new(s))),
                    Err(e) => {
                        let _ = tx.send(OrchestratorEvent::AuthFailed(e)).await;
                        return;
                    }
                }
            } else {
                None
            };

            let _ = tx.send(OrchestratorEvent::AuthSucceeded).await;

            let (be_tx, be_rx) = async_channel::unbounded::<BackendEvent>();

            let tx_fwd = tx.clone();
            let fwd_handle = tokio::spawn(async move {
                while let Ok(event) = be_rx.recv().await {
                    let BackendEvent::LogLine(k, line) = event;
                    let _ = tx_fwd.send(OrchestratorEvent::BackendLog(k, line)).await;
                }
            });

            for backend in &backends {
                let kind = backend.kind();
                let _ = tx
                    .send(OrchestratorEvent::BackendStarted(kind.clone()))
                    .await;
                let runner = CommandRunner::new(be_tx.clone(), kind.clone(), shell.clone());
                let result = backend.run_cleanup(&runner).await;
                let _ = tx
                    .send(OrchestratorEvent::BackendFinished(kind, result))
                    .await;
            }

            drop(be_tx);
            let _ = fwd_handle.await;

            if let Some(s) = shell {
                s.lock().await.close().await;
            }

            let _ = tx.send(OrchestratorEvent::AllFinished).await;
        });
    }
}
