use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Handle to a running operation, supporting cancellation.
pub struct OperationHandle {
    pub operation_id: String,
    pub backend_id: String,
    pub cancel_token: CancellationToken,
    pub join_handle: Option<JoinHandle<()>>,
}

impl OperationHandle {
    /// Cancel the operation with graceful SIGTERM → SIGKILL escalation.
    /// Returns true if cancellation was successfully initiated.
    pub async fn cancel(&mut self) -> bool {
        if self.cancel_token.is_cancelled() {
            return false;
        }
        self.cancel_token.cancel();
        true
    }

    /// Whether this operation can be cancelled.
    pub fn is_cancellable(&self) -> bool {
        !self.cancel_token.is_cancelled()
    }
}

/// Send SIGTERM to the process group of the given child.
#[cfg(unix)]
pub fn kill_process_group(child: &tokio::process::Child) {
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
    }
}

#[cfg(not(unix))]
pub fn kill_process_group(_child: &tokio::process::Child) {
    // No-op on non-Unix platforms (daemon is Linux-only)
}

/// Send SIGKILL to the process group of the given child (forced kill).
#[cfg(unix)]
pub fn kill_process_group_force(child: &tokio::process::Child) {
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
pub fn kill_process_group_force(_child: &tokio::process::Child) {
    // No-op on non-Unix platforms
}
