use crate::allowlist::CommandAllowlist;
use crate::audit;
use crate::auth;
use crate::cancel::OperationHandle;
use crate::executor;
use crate::lifecycle::IdleTracker;

use log::info;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;
use zbus::{fdo, interface, object_server::SignalEmitter};

/// Maximum number of concurrent operations allowed.
const MAX_CONCURRENT_OPS: usize = 4;

pub struct UpDaemon {
    operations: Arc<Mutex<HashMap<String, OperationHandle>>>,
    allowlist: CommandAllowlist,
    idle_tracker: Arc<Mutex<IdleTracker>>,
}

impl UpDaemon {
    pub fn new(idle_tracker: Arc<Mutex<IdleTracker>>) -> Self {
        Self {
            operations: Arc::new(Mutex::new(HashMap::new())),
            allowlist: CommandAllowlist::default(),
            idle_tracker,
        }
    }
}

#[interface(name = "io.github.up.Daemon1")]
impl UpDaemon {
    /// Run the update command for a specific backend.
    /// Returns an operation_id that can be used to track progress and cancel.
    async fn run_update(
        &self,
        backend_id: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> fdo::Result<String> {
        let sender = header
            .sender()
            .ok_or_else(|| fdo::Error::Failed("Missing sender in D-Bus header".into()))?
            .to_string();

        // Check polkit authorization
        let action = "io.github.up.update.system";
        if !auth::check_polkit(connection, &sender, action)
            .await
            .map_err(|e| fdo::Error::Failed(format!("Polkit check failed: {}", e)))?
        {
            return Err(fdo::Error::AccessDenied("Authorization denied".into()));
        }

        // Validate backend against allowlist
        let commands = self
            .allowlist
            .get_update_commands(backend_id)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("Unknown backend: {}", backend_id)))?;

        // Check concurrent operation limit
        let ops = self.operations.lock().await;
        if ops.len() >= MAX_CONCURRENT_OPS {
            return Err(fdo::Error::LimitsExceeded(
                "Too many concurrent operations".into(),
            ));
        }
        drop(ops);

        let operation_id = Uuid::new_v4().to_string();

        // Audit log
        audit::log_operation_start(&sender, action, backend_id, &operation_id);

        // Mark activity
        self.idle_tracker.lock().await.mark_active();

        // Spawn the operation
        let op_id = operation_id.clone();

        let handle = executor::spawn_operation(
            operation_id.clone(),
            backend_id.to_string(),
            commands,
            emitter.connection().clone(),
        )
        .await;

        self.operations.lock().await.insert(op_id.clone(), handle);

        // Spawn cleanup task to remove operation from map when it completes
        let ops_ref = self.operations.clone();
        let cleanup_id = op_id.clone();
        tokio::spawn(async move {
            // Wait a short moment for the operation to register, then poll for completion
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let mut ops = ops_ref.lock().await;
                if let Some(handle) = ops.get(&cleanup_id) {
                    if handle.join_handle.as_ref().is_none_or(|h| h.is_finished()) {
                        ops.remove(&cleanup_id);
                        break;
                    }
                } else {
                    break;
                }
            }
        });

        Ok(operation_id)
    }

    /// Run the cleanup command for a specific backend.
    async fn run_cleanup(
        &self,
        backend_id: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> fdo::Result<String> {
        let sender = header
            .sender()
            .ok_or_else(|| fdo::Error::Failed("Missing sender in D-Bus header".into()))?
            .to_string();

        let action = "io.github.up.cleanup.system";
        if !auth::check_polkit(connection, &sender, action)
            .await
            .map_err(|e| fdo::Error::Failed(format!("Polkit check failed: {}", e)))?
        {
            return Err(fdo::Error::AccessDenied("Authorization denied".into()));
        }

        let commands = self
            .allowlist
            .get_cleanup_commands(backend_id)
            .ok_or_else(|| {
                fdo::Error::InvalidArgs(format!("No cleanup command for backend: {}", backend_id))
            })?;

        let ops = self.operations.lock().await;
        if ops.len() >= MAX_CONCURRENT_OPS {
            return Err(fdo::Error::LimitsExceeded(
                "Too many concurrent operations".into(),
            ));
        }
        drop(ops);

        let operation_id = Uuid::new_v4().to_string();
        audit::log_operation_start(&sender, action, backend_id, &operation_id);
        self.idle_tracker.lock().await.mark_active();

        let handle = executor::spawn_operation(
            operation_id.clone(),
            backend_id.to_string(),
            commands,
            emitter.connection().clone(),
        )
        .await;

        self.operations
            .lock()
            .await
            .insert(operation_id.clone(), handle);

        // Spawn cleanup task to remove operation from map when it completes
        let ops_ref = self.operations.clone();
        let cleanup_id = operation_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let mut ops = ops_ref.lock().await;
                if let Some(handle) = ops.get(&cleanup_id) {
                    if handle.join_handle.as_ref().is_none_or(|h| h.is_finished()) {
                        ops.remove(&cleanup_id);
                        break;
                    }
                } else {
                    break;
                }
            }
        });

        Ok(operation_id)
    }

    /// Run a distribution upgrade.
    async fn run_upgrade(
        &self,
        distro_id: &str,
        variant: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> fdo::Result<String> {
        let sender = header
            .sender()
            .ok_or_else(|| fdo::Error::Failed("Missing sender in D-Bus header".into()))?
            .to_string();

        let action = "io.github.up.upgrade.system";
        if !auth::check_polkit(connection, &sender, action)
            .await
            .map_err(|e| fdo::Error::Failed(format!("Polkit check failed: {}", e)))?
        {
            return Err(fdo::Error::AccessDenied("Authorization denied".into()));
        }

        let commands = self
            .allowlist
            .get_upgrade_commands(distro_id, variant)
            .ok_or_else(|| {
                fdo::Error::InvalidArgs(format!("No upgrade path for {}/{}", distro_id, variant))
            })?;

        let operation_id = Uuid::new_v4().to_string();
        audit::log_operation_start(&sender, action, distro_id, &operation_id);
        self.idle_tracker.lock().await.mark_active();

        let handle = executor::spawn_operation(
            operation_id.clone(),
            distro_id.to_string(),
            commands,
            emitter.connection().clone(),
        )
        .await;

        self.operations
            .lock()
            .await
            .insert(operation_id.clone(), handle);

        // Spawn cleanup task to remove operation from map when it completes
        let ops_ref = self.operations.clone();
        let cleanup_id = operation_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let mut ops = ops_ref.lock().await;
                if let Some(handle) = ops.get(&cleanup_id) {
                    if handle.join_handle.as_ref().is_none_or(|h| h.is_finished()) {
                        ops.remove(&cleanup_id);
                        break;
                    }
                } else {
                    break;
                }
            }
        });

        Ok(operation_id)
    }

    /// Create a pre-update snapshot.
    async fn create_snapshot(
        &self,
        tool: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> fdo::Result<String> {
        let sender = header
            .sender()
            .ok_or_else(|| fdo::Error::Failed("Missing sender in D-Bus header".into()))?
            .to_string();

        let action = "io.github.up.snapshot.create";
        if !auth::check_polkit(connection, &sender, action)
            .await
            .map_err(|e| fdo::Error::Failed(format!("Polkit check failed: {}", e)))?
        {
            return Err(fdo::Error::AccessDenied("Authorization denied".into()));
        }

        let commands = self
            .allowlist
            .get_snapshot_commands(tool)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("Unknown snapshot tool: {}", tool)))?;

        let operation_id = Uuid::new_v4().to_string();
        audit::log_operation_start(&sender, action, tool, &operation_id);
        self.idle_tracker.lock().await.mark_active();

        let handle = executor::spawn_operation(
            operation_id.clone(),
            tool.to_string(),
            commands,
            emitter.connection().clone(),
        )
        .await;

        self.operations
            .lock()
            .await
            .insert(operation_id.clone(), handle);

        // Spawn cleanup task to remove operation from map when it completes
        let ops_ref = self.operations.clone();
        let cleanup_id = operation_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let mut ops = ops_ref.lock().await;
                if let Some(handle) = ops.get(&cleanup_id) {
                    if handle.join_handle.as_ref().is_none_or(|h| h.is_finished()) {
                        ops.remove(&cleanup_id);
                        break;
                    }
                } else {
                    break;
                }
            }
        });

        Ok(operation_id)
    }

    /// Cancel a running operation. Returns true if cancellation was initiated.
    async fn cancel(
        &self,
        operation_id: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> fdo::Result<bool> {
        let sender = header
            .sender()
            .ok_or_else(|| fdo::Error::Failed("Missing sender in D-Bus header".into()))?
            .to_string();

        // Check polkit authorization for cancel
        let action = "io.github.up.cancel.operation";
        if !auth::check_polkit(connection, &sender, action)
            .await
            .map_err(|e| fdo::Error::Failed(format!("Polkit check failed: {}", e)))?
        {
            return Err(fdo::Error::AccessDenied("Authorization denied".into()));
        }

        let mut ops = self.operations.lock().await;
        if let Some(handle) = ops.get_mut(operation_id) {
            let result = handle.cancel().await;
            if result {
                info!("Operation {} cancelled", operation_id);
                audit::log_operation_cancelled(operation_id);
            }
            Ok(result)
        } else {
            Ok(false)
        }
    }

    /// List currently active operations.
    /// Returns array of (operation_id, backend_id, is_cancellable).
    async fn list_operations(&self) -> fdo::Result<Vec<(String, String, bool)>> {
        let ops = self.operations.lock().await;
        let list: Vec<(String, String, bool)> = ops
            .values()
            .map(|h| {
                (
                    h.operation_id.clone(),
                    h.backend_id.clone(),
                    h.is_cancellable(),
                )
            })
            .collect();
        Ok(list)
    }

    /// Signal: a line of output from an operation.
    #[zbus(signal)]
    pub async fn operation_output(
        emitter: &SignalEmitter<'_>,
        operation_id: &str,
        line: &str,
    ) -> zbus::Result<()>;

    /// Signal: an operation has completed.
    #[zbus(signal)]
    pub async fn operation_complete(
        emitter: &SignalEmitter<'_>,
        operation_id: &str,
        success: bool,
        exit_code: i32,
        summary: &str,
    ) -> zbus::Result<()>;

    /// The daemon version.
    #[zbus(property)]
    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    /// Number of currently active operations.
    #[zbus(property)]
    async fn active_operation_count(&self) -> u32 {
        self.operations.lock().await.len() as u32
    }

    /// Configured idle timeout in seconds.
    #[zbus(property)]
    fn idle_timeout_secs(&self) -> u32 {
        60
    }
}
