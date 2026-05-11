//! D-Bus client for communicating with the `up-daemon` privileged service.
//!
//! Provides [`DaemonExecutor`] which implements [`CommandExecutor`] by routing
//! commands through the D-Bus daemon instead of spawning privileged processes
//! directly. Falls back to the legacy `pkexec` path when the daemon is unavailable.

use crate::backends::BackendError;
use crate::executor::CommandExecutor;
use log::{info, warn};
use std::future::Future;
use std::pin::Pin;

/// zbus proxy for the `io.github.up.Daemon1` interface.
#[zbus::proxy(
    interface = "io.github.up.Daemon1",
    default_service = "io.github.up.Daemon",
    default_path = "/io/github/up/Daemon"
)]
trait UpDaemon {
    /// Run the update command for a specific backend.
    async fn run_update(&self, backend_id: &str) -> zbus::Result<String>;

    /// Run the cleanup command for a specific backend.
    async fn run_cleanup(&self, backend_id: &str) -> zbus::Result<String>;

    /// Run a distribution upgrade.
    async fn run_upgrade(&self, distro_id: &str, variant: &str) -> zbus::Result<String>;

    /// Create a pre-update snapshot.
    async fn create_snapshot(&self, tool: &str) -> zbus::Result<String>;

    /// Cancel a running operation.
    async fn cancel(&self, operation_id: &str) -> zbus::Result<bool>;

    /// List currently active operations.
    async fn list_operations(&self) -> zbus::Result<Vec<(String, String, bool)>>;

    /// Signal: a line of output from an operation.
    #[zbus(signal)]
    async fn operation_output(operation_id: String, line: String);

    /// Signal: an operation has completed.
    #[zbus(signal)]
    async fn operation_complete(
        operation_id: String,
        success: bool,
        exit_code: i32,
        summary: String,
    );

    /// The daemon version.
    #[zbus(property)]
    fn version(&self) -> zbus::Result<String>;

    /// Number of currently active operations.
    #[zbus(property)]
    fn active_operation_count(&self) -> zbus::Result<u32>;
}

/// The execution mode for privileged operations.
#[allow(dead_code)]
pub enum ExecutionMode {
    /// Use the D-Bus daemon for privileged operations.
    Daemon(zbus::Connection),
    /// Fall back to the legacy pkexec path.
    LegacyPkexec,
}

/// Determine the execution strategy for privileged operations.
///
/// Attempts to connect to the `up-daemon` on the system bus. If the daemon
/// is responsive, returns [`ExecutionMode::Daemon`]. Otherwise falls back to
/// [`ExecutionMode::LegacyPkexec`].
#[allow(dead_code)]
pub async fn detect_execution_mode() -> ExecutionMode {
    match zbus::Connection::system().await {
        Ok(conn) => {
            match UpDaemonProxy::new(&conn).await {
                Ok(proxy) => {
                    // Verify daemon is responsive by checking the version property
                    match proxy.version().await {
                        Ok(version) => {
                            info!("Connected to up-daemon v{}", version);
                            ExecutionMode::Daemon(conn)
                        }
                        Err(e) => {
                            warn!("up-daemon not responsive: {}", e);
                            ExecutionMode::LegacyPkexec
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to create daemon proxy: {}", e);
                    ExecutionMode::LegacyPkexec
                }
            }
        }
        Err(e) => {
            warn!("Cannot connect to system bus: {}", e);
            ExecutionMode::LegacyPkexec
        }
    }
}

/// A [`CommandExecutor`] implementation that routes commands through the D-Bus daemon.
///
/// This executor starts an update operation via D-Bus and collects output from
/// the `OperationOutput` signal, returning the collected output on completion.
#[allow(dead_code)]
pub struct DaemonExecutor {
    connection: zbus::Connection,
    backend_id: String,
    /// Channel sender for streaming output lines to the orchestrator in real time.
    output_tx: Option<async_channel::Sender<String>>,
}

#[allow(dead_code)]
impl DaemonExecutor {
    /// Create a new `DaemonExecutor` for the given backend.
    pub fn new(connection: zbus::Connection, backend_id: String) -> Self {
        Self {
            connection,
            backend_id,
            output_tx: None,
        }
    }

    /// Create with an output channel for real-time line streaming.
    pub fn with_output_channel(
        connection: zbus::Connection,
        backend_id: String,
        tx: async_channel::Sender<String>,
    ) -> Self {
        Self {
            connection,
            backend_id,
            output_tx: Some(tx),
        }
    }

    /// Get the proxy for the daemon.
    async fn proxy(&self) -> Result<UpDaemonProxy<'_>, BackendError> {
        UpDaemonProxy::new(&self.connection)
            .await
            .map_err(|e| BackendError::Spawn(format!("Failed to create daemon proxy: {}", e)))
    }
}

impl CommandExecutor for DaemonExecutor {
    fn run<'a>(
        &'a self,
        _program: &'a str,
        _args: &'a [&'a str],
    ) -> Pin<Box<dyn Future<Output = Result<String, BackendError>> + Send + 'a>> {
        Box::pin(async move {
            let proxy = self.proxy().await?;

            // Start the update operation via D-Bus
            let operation_id = proxy.run_update(&self.backend_id).await.map_err(|e| {
                let msg = e.to_string();
                if msg.contains("Authorization denied") || msg.contains("AccessDenied") {
                    BackendError::AuthCancelled
                } else {
                    BackendError::Spawn(format!("D-Bus call failed: {}", msg))
                }
            })?;

            // Subscribe to signals for this operation
            let mut output_stream = proxy.receive_operation_output().await.map_err(|e| {
                BackendError::Spawn(format!("Failed to subscribe to signals: {}", e))
            })?;

            let mut complete_stream = proxy.receive_operation_complete().await.map_err(|e| {
                BackendError::Spawn(format!("Failed to subscribe to completion: {}", e))
            })?;

            let mut collected_output = String::new();

            // Process signals until operation completes
            use futures_util::StreamExt;
            loop {
                tokio::select! {
                    Some(signal) = output_stream.next() => {
                        if let Ok(args) = signal.args() {
                            if args.operation_id == operation_id {
                                collected_output.push_str(&args.line);
                                collected_output.push('\n');
                                if let Some(tx) = &self.output_tx {
                                    let _ = tx.send(args.line.to_string()).await;
                                }
                            }
                        }
                    }
                    Some(signal) = complete_stream.next() => {
                        if let Ok(args) = signal.args() {
                            if args.operation_id == operation_id {
                                if args.success {
                                    return Ok(collected_output);
                                } else {
                                    return Err(BackendError::Exit {
                                        code: args.exit_code,
                                        message: args.summary.to_string(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        })
    }
}

/// Cancel an operation via the daemon.
#[allow(dead_code)]
pub async fn cancel_operation(connection: &zbus::Connection, operation_id: &str) -> bool {
    match UpDaemonProxy::new(connection).await {
        Ok(proxy) => proxy.cancel(operation_id).await.unwrap_or(false),
        Err(_) => false,
    }
}
