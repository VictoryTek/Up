use crate::backends::BackendKind;
use log::{info, warn};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;

// ── Persistent privileged shell ──────────────────────────────────────────

/// A long-lived `pkexec sh` process that accepts commands on stdin.
///
/// The user authenticates exactly once (when the process spawns).  Every
/// subsequent command written to stdin runs with root privileges without
/// triggering another polkit prompt.
pub struct PrivilegedShell {
    child: tokio::process::Child,
    stdin: Option<tokio::process::ChildStdin>,
    reader: BufReader<tokio::process::ChildStdout>,
}

/// Sentinel that marks the end of a command's output inside the shell.
const RC_MARKER: &str = "___UP_RC_";

impl PrivilegedShell {
    /// Spawn `pkexec sh` and verify that authentication succeeded.
    pub async fn new() -> Result<Self, String> {
        let mut child = tokio::process::Command::new("pkexec")
            .arg("/bin/sh")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("Failed to start pkexec: {e}"))?;

        let stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or("No stdout from pkexec")?;
        let reader = BufReader::new(stdout);

        let mut shell = Self {
            child,
            stdin,
            reader,
        };

        // Write a trivial command; if auth was cancelled or pkexec failed the
        // process will have already exited so read_line returns 0 bytes.
        let s = shell.stdin.as_mut().ok_or("No stdin for shell")?;
        s.write_all(b"echo '___UP_READY___'\n")
            .await
            .map_err(|e| format!("Failed to write to shell: {e}"))?;
        s.flush()
            .await
            .map_err(|e| format!("Failed to flush shell: {e}"))?;

        let mut line = String::new();
        let n = shell
            .reader
            .read_line(&mut line)
            .await
            .map_err(|e| format!("Failed to read from shell: {e}"))?;

        if n == 0 {
            // Process exited before responding — authentication was cancelled or denied.
            let status = shell.child.wait().await.ok();
            let code = status.and_then(|s| s.code()).unwrap_or(-1);
            return Err(format!("Authentication cancelled (exit code {code})"));
        }

        if line.trim() != "___UP_READY___" {
            return Err(format!("Unexpected shell response: {:?}", line.trim()));
        }

        Ok(shell)
    }

    /// Execute a command inside the elevated shell, streaming output
    /// line-by-line through `tx`.  Returns the full captured output on
    /// success, or an error string on non-zero exit.
    pub async fn run_command(
        &mut self,
        args: &[&str],
        tx: &async_channel::Sender<(BackendKind, String)>,
        kind: BackendKind,
    ) -> Result<String, String> {
        let cmd_line = args
            .iter()
            .map(|a| shell_quote(a))
            .collect::<Vec<_>>()
            .join(" ");

        // Run the command with stderr merged into stdout, then print a
        // sentinel carrying the exit code so we know where output ends.
        let script = format!("{cmd_line} 2>&1\necho '{RC_MARKER}'$?'___'\n");
        let s = self.stdin.as_mut().ok_or("Shell stdin closed")?;
        s.write_all(script.as_bytes())
            .await
            .map_err(|e| format!("Failed to write command: {e}"))?;
        s.flush()
            .await
            .map_err(|e| format!("Failed to flush command: {e}"))?;

        let mut full_output = String::new();
        loop {
            let mut line = String::new();
            let n = self
                .reader
                .read_line(&mut line)
                .await
                .map_err(|e| format!("Failed to read output: {e}"))?;
            if n == 0 {
                return Err("Privileged shell closed unexpectedly".to_string());
            }
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix(RC_MARKER) {
                if let Some(code_str) = rest.strip_suffix("___") {
                    let code: i32 = code_str.parse().unwrap_or(-1);
                    if code == 0 {
                        return Ok(full_output);
                    }
                    return Err(format!("Command exited with code {code}"));
                }
            }
            let content = line.trim_end_matches('\n').to_string();
            full_output.push_str(&content);
            full_output.push('\n');
            let _ = tx.send((kind, content)).await;
        }
    }

    /// Cleanly shut down the privileged shell.
    pub async fn close(&mut self) {
        // Dropping stdin sends EOF, causing sh to exit.
        self.stdin.take();
        let _ = self.child.wait().await;
    }
}

/// Quote a string for safe interpolation inside a POSIX shell command line.
fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if s.bytes().all(|b| {
        b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'_' | b'/' | b'.' | b'=' | b':' | b'+' | b',')
    }) {
        s.to_string()
    } else {
        // Wrap in single quotes; escape embedded single quotes.
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

// ── CommandRunner ────────────────────────────────────────────────────────

/// Runs system commands and streams output back to the UI via an async channel.
#[derive(Clone)]
pub struct CommandRunner {
    tx: async_channel::Sender<(BackendKind, String)>,
    kind: BackendKind,
    shell: Option<Arc<Mutex<PrivilegedShell>>>,
}

impl CommandRunner {
    pub fn new(
        tx: async_channel::Sender<(BackendKind, String)>,
        kind: BackendKind,
        shell: Option<Arc<Mutex<PrivilegedShell>>>,
    ) -> Self {
        Self { tx, kind, shell }
    }

    /// Run a command, streaming stdout/stderr line by line. Returns full output on success.
    ///
    /// If `program` is `"pkexec"` and a [`PrivilegedShell`] was provided at
    /// construction time, the command is routed through the already-elevated
    /// shell instead of spawning a new `pkexec` process.
    pub async fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
        let display_cmd = format!("{} {}", program, args.join(" "));
        self.send(format!("$ {display_cmd}")).await;
        info!("Running: {} {:?}", program, args);

        // Route pkexec calls through the pre-authenticated shell if available.
        if program == "pkexec" {
            if let Some(shell) = &self.shell {
                let mut guard = shell.lock().await;
                return guard.run_command(args, &self.tx, self.kind).await;
            }
        }

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
pub fn run_command_sync(program: &str, args: &[&str], tx: &async_channel::Sender<String>) -> bool {
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
