use crate::backends::BackendKind;
use log::{info, warn};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Runs system commands and streams output back to the UI via an async channel.
#[derive(Clone)]
pub struct CommandRunner {
    tx: async_channel::Sender<(BackendKind, String)>,
    kind: BackendKind,
}

impl CommandRunner {
    pub fn new(tx: async_channel::Sender<(BackendKind, String)>, kind: BackendKind) -> Self {
        Self { tx, kind }
    }

    /// Run a command, streaming stdout/stderr line by line. Returns full output on success.
    pub async fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
        let display_cmd = format!("{} {}", program, args.join(" "));
        self.send(format!("$ {display_cmd}")).await;
        info!("Running: {} {:?}", program, args);

        let mut child = Command::new(program)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to start {program}: {e}"))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // Read stdout and stderr concurrently to avoid pipe-buffer deadlocks.
        // If one pipe fills its kernel buffer while we are draining the other,
        // the child process blocks and we never reach EOF on either pipe.
        let tx_stdout = self.tx.clone();
        let kind_stdout = self.kind;
        let stdout_task = async move {
            let mut out = String::new();
            if let Some(pipe) = stdout {
                let mut reader = BufReader::new(pipe).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    out.push_str(&line);
                    out.push('\n');
                    let _ = tx_stdout.send((kind_stdout, line)).await;
                }
            }
            out
        };

        let tx_stderr = self.tx.clone();
        let kind_stderr = self.kind;
        let stderr_task = async move {
            let mut out = String::new();
            if let Some(pipe) = stderr {
                let mut reader = BufReader::new(pipe).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    out.push_str(&line);
                    out.push('\n');
                    let _ = tx_stderr.send((kind_stderr, line)).await;
                }
            }
            out
        };

        let (stdout_output, stderr_output) = tokio::join!(stdout_task, stderr_task);
        let full_output = stdout_output + &stderr_output;

        let status = child
            .wait()
            .await
            .map_err(|e| format!("Failed to wait for {program}: {e}"))?;

        if status.success() {
            Ok(full_output)
        } else {
            let code = status.code().unwrap_or(-1);
            warn!("{program} exited with code {code}");
            Err(format!("{program} exited with code {code}"))
        }
    }

    async fn send(&self, msg: String) {
        let _ = self.tx.send((self.kind, msg)).await;
    }
}

/// Spawns `program` with `args`, drains stdout and stderr concurrently on
/// blocking threads, forwards each line to `tx`, and returns `true` on
/// success (exit code 0) or `false` on any failure (spawn error, non-zero
/// exit, or pipe error).
///
/// Unlike [`CommandRunner::run`], this function is synchronous and creates
/// its own blocking thread pair for stdout/stderr draining.  It is intended
/// for use in `std::thread::spawn` contexts (such as the upgrade workflow)
/// where no async executor is available.
///
/// Thread panics during drain are detected and reported as failures instead
/// of being silently discarded.
pub fn run_command_sync(
    program: &str,
    args: &[&str],
    tx: &async_channel::Sender<String>,
) -> bool {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let result = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    match result {
        Ok(mut child) => {
            // Drain stdout and stderr concurrently on separate threads to prevent
            // pipe-buffer deadlock. If one pipe fills its kernel buffer (~64 KiB)
            // while the parent is draining the other, the child blocks and neither
            // pipe ever reaches EOF — causing the parent to hang indefinitely.
            let stdout_pipe = child.stdout.take();
            let stderr_pipe = child.stderr.take();

            let tx_stdout = tx.clone();
            let stdout_thread = std::thread::spawn(move || {
                if let Some(pipe) = stdout_pipe {
                    let reader = BufReader::new(pipe);
                    for line in reader.lines().map_while(Result::ok) {
                        let _ = tx_stdout.send_blocking(line);
                    }
                }
            });

            let tx_stderr = tx.clone();
            let stderr_thread = std::thread::spawn(move || {
                if let Some(pipe) = stderr_pipe {
                    let reader = BufReader::new(pipe);
                    for line in reader.lines().map_while(Result::ok) {
                        let _ = tx_stderr.send_blocking(format!("stderr: {line}"));
                    }
                }
            });

            // Detect thread panics instead of silently discarding them.
            if stdout_thread.join().is_err() {
                let _ = tx.send_blocking(format!(
                    "Internal error: stdout drain thread panicked for {program}"
                ));
            }
            if stderr_thread.join().is_err() {
                let _ = tx.send_blocking(format!(
                    "Internal error: stderr drain thread panicked for {program}"
                ));
            }

            match child.wait() {
                Ok(status) => {
                    if status.success() {
                        let _ = tx.send_blocking("Command completed successfully.".into());
                        true
                    } else {
                        let code = status.code().unwrap_or(-1);
                        let _ = tx.send_blocking(format!("Command exited with code {code}"));
                        false
                    }
                }
                Err(e) => {
                    let _ = tx.send_blocking(format!("Failed to wait for process: {e}"));
                    false
                }
            }
        }
        Err(e) => {
            let _ = tx.send_blocking(format!("Failed to start {program}: {e}"));
            false
        }
    }
}
