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
