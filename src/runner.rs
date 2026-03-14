use crate::backends::BackendKind;
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

        let mut child = Command::new(program)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to start {program}: {e}"))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let mut full_output = String::new();

        // Read stdout
        if let Some(out) = stdout {
            let mut reader = BufReader::new(out).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                full_output.push_str(&line);
                full_output.push('\n');
                self.send(line).await;
            }
        }

        // Read stderr
        if let Some(err) = stderr {
            let mut reader = BufReader::new(err).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                full_output.push_str(&line);
                full_output.push('\n');
                self.send(format!("stderr: {line}")).await;
            }
        }

        let status = child
            .wait()
            .await
            .map_err(|e| format!("Failed to wait for {program}: {e}"))?;

        if status.success() {
            Ok(full_output)
        } else {
            let code = status.code().unwrap_or(-1);
            Err(format!("{program} exited with code {code}"))
        }
    }

    async fn send(&self, msg: String) {
        let _ = self.tx.send((self.kind, msg)).await;
    }
}
