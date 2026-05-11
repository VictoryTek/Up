use log::info;

/// Log the start of a privileged operation to the systemd journal.
pub fn log_operation_start(caller: &str, action: &str, backend: &str, operation_id: &str) {
    info!(
        target: "up-daemon::audit",
        "AUDIT: START caller={} action={} backend={} op_id={}",
        caller, action, backend, operation_id
    );
}

/// Log a completed operation to the systemd journal.
pub fn log_operation_complete(operation_id: &str, success: bool, exit_code: i32) {
    info!(
        target: "up-daemon::audit",
        "AUDIT: COMPLETE op_id={} success={} exit_code={}",
        operation_id, success, exit_code
    );
}

/// Log a cancelled operation.
pub fn log_operation_cancelled(operation_id: &str) {
    info!(
        target: "up-daemon::audit",
        "AUDIT: CANCELLED op_id={}",
        operation_id
    );
}
