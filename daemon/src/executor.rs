use crate::cancel::OperationHandle;
use crate::interface::UpDaemon;

use log::{error, info};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use zbus::Connection;

/// A command definition resolved from the allowlist, ready to execute.
#[derive(Debug, Clone)]
pub struct ResolvedCommand {
    pub program: String,
    pub args: Vec<String>,
    pub environment: Vec<(String, String)>,
}

/// Spawn a privileged operation in a new process group.
/// Streams output lines via the OperationOutput D-Bus signal.
/// Emits OperationComplete when done.
pub async fn spawn_operation(
    operation_id: String,
    backend_id: String,
    commands: Vec<ResolvedCommand>,
    connection: Connection,
) -> OperationHandle {
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let token_clone = cancel_token.clone();
    let op_id = operation_id.clone();
    let be_id = backend_id.clone();

    let join_handle = tokio::spawn(async move {
        let mut overall_success = true;
        let mut overall_exit_code = 0i32;
        let mut summary_lines: Vec<String> = Vec::new();

        for resolved_cmd in &commands {
            if token_clone.is_cancelled() {
                overall_success = false;
                summary_lines.push("Cancelled".to_string());
                break;
            }

            info!(
                "Executing: {} {:?} (op={})",
                resolved_cmd.program, resolved_cmd.args, op_id
            );

            let mut cmd = Command::new(&resolved_cmd.program);
            cmd.args(&resolved_cmd.args)
                .envs(
                    resolved_cmd
                        .environment
                        .iter()
                        .map(|(k, v)| (k.as_str(), v.as_str())),
                )
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .stdin(Stdio::null());
            // Create a new process group so we can signal the entire group
            #[cfg(unix)]
            {
                cmd.process_group(0);
            }
            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(e) => {
                    error!("Failed to spawn {}: {}", resolved_cmd.program, e);
                    overall_success = false;
                    overall_exit_code = -1;
                    summary_lines.push(format!("Failed to spawn {}: {}", resolved_cmd.program, e));
                    break;
                }
            };

            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            // Stream stdout lines as OperationOutput signals
            let op_id_clone = op_id.clone();
            let conn_clone = connection.clone();
            let stdout_task = tokio::spawn(async move {
                if let Some(stdout) = stdout {
                    let reader = BufReader::new(stdout);
                    let mut lines = reader.lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        // Emit D-Bus signal (best-effort)
                        let iface_ref = conn_clone
                            .object_server()
                            .interface::<_, UpDaemon>("/io/github/up/Daemon")
                            .await;
                        if let Ok(iface) = iface_ref {
                            let _ = UpDaemon::operation_output(
                                iface.signal_emitter(),
                                &op_id_clone,
                                &line,
                            )
                            .await;
                        }
                    }
                }
            });

            // Stream stderr lines as well
            let op_id_clone2 = op_id.clone();
            let conn_clone2 = connection.clone();
            let stderr_task = tokio::spawn(async move {
                if let Some(stderr) = stderr {
                    let reader = BufReader::new(stderr);
                    let mut lines = reader.lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let iface_ref = conn_clone2
                            .object_server()
                            .interface::<_, UpDaemon>("/io/github/up/Daemon")
                            .await;
                        if let Ok(iface) = iface_ref {
                            let _ = UpDaemon::operation_output(
                                iface.signal_emitter(),
                                &op_id_clone2,
                                &line,
                            )
                            .await;
                        }
                    }
                }
            });

            // Wait for the child process
            let status = tokio::select! {
                status = child.wait() => status,
                _ = token_clone.cancelled() => {
                    // Cancellation requested — send SIGTERM to process group
                    crate::cancel::kill_process_group(&child);
                    // Wait briefly for graceful exit
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        child.wait(),
                    ).await {
                        Ok(status) => status,
                        Err(_) => {
                            // Escalate to SIGKILL
                            crate::cancel::kill_process_group_force(&child);
                            child.wait().await
                        }
                    }
                }
            };

            let _ = stdout_task.await;
            let _ = stderr_task.await;

            match status {
                Ok(exit) => {
                    let code = exit.code().unwrap_or(-1);
                    if code != 0 {
                        overall_success = false;
                        overall_exit_code = code;
                        summary_lines.push(format!(
                            "{} exited with code {}",
                            resolved_cmd.program, code
                        ));
                    }
                }
                Err(e) => {
                    overall_success = false;
                    overall_exit_code = -1;
                    summary_lines.push(format!("Process error: {}", e));
                }
            }
        }

        // Emit completion signal
        let summary = if summary_lines.is_empty() {
            "Completed successfully".to_string()
        } else {
            summary_lines.join("; ")
        };

        // Audit log the completion
        crate::audit::log_operation_complete(&op_id, overall_success, overall_exit_code);

        let iface_ref = connection
            .object_server()
            .interface::<_, UpDaemon>("/io/github/up/Daemon")
            .await;
        if let Ok(iface) = iface_ref {
            let _ = UpDaemon::operation_complete(
                iface.signal_emitter(),
                &op_id,
                overall_success,
                overall_exit_code,
                &summary,
            )
            .await;
        }

        info!(
            "Operation {} ({}) completed: success={}, exit_code={}",
            op_id, be_id, overall_success, overall_exit_code
        );
    });

    OperationHandle {
        operation_id,
        backend_id,
        cancel_token,
        join_handle: Some(join_handle),
    }
}
