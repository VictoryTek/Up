use crate::backends::{BackendKind, UpdateResult};
use log::{info, warn};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;

/// Unified event type that carries all backend activity through a single ordered channel.
/// The worker sends events in strict sequence: Started → LogLine(s) → Finished.
/// The GTK receive loop processes one event at a time, eliminating any race between
/// independent futures on separate channels.
#[derive(Debug)]
pub enum BackendEvent {
    /// The named backend has started its update operation.
    Started(BackendKind),
    /// A single line of log output produced by the named backend.
    LogLine(BackendKind, String),
    /// The named backend has finished; carries its result.
    Finished(BackendKind, UpdateResult),
}

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
    /// Unique token included in every sentinel for this session.
    /// Prevents any subprocess from spoofing exit-code markers by guessing
    /// the fixed compile-time constant.
    session_id: String,
}

/// Maximum wall-clock time a single privileged command may run.
/// Commands that exceed this limit return an error; the shell is closed.
const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3600); // 1 hour

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

        let pid = std::process::id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let session_id = format!("{:x}_{:x}", pid, ts);

        let mut shell = Self {
            child,
            stdin,
            reader,
            session_id,
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
            let reason = match code {
                126 => "authentication was cancelled".to_string(),
                127 => "not authorised or pkexec not found".to_string(),
                _ => format!("exit code {code}"),
            };
            return Err(format!("pkexec failed: {reason}"));
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
        tx: &async_channel::Sender<BackendEvent>,
        kind: BackendKind,
    ) -> Result<String, String> {
        // Reject arguments containing control characters that could be interpreted
        // by the root shell as command separators or terminators.
        for arg in args {
            if arg.contains(['\n', '\r', '\0']) {
                return Err(format!(
                    "Security: argument contains forbidden control character: {:?}",
                    arg
                ));
            }
        }

        let cmd_line = args
            .iter()
            .map(|a| shell_quote(a))
            .collect::<Vec<_>>()
            .join(" ");

        // Build per-session sentinel strings; unpredictable to any subprocess.
        let rc_prefix = format!("___UP_RC_{}_", self.session_id);
        let rc_suffix = "___";

        // Run the command with stderr merged into stdout, then print a
        // sentinel carrying the exit code so we know where output ends.
        // Use printf instead of echo to avoid interpretation of special flags.
        let script = format!(
            "{cmd_line} 2>&1\nprintf '%s%d%s\\n' '{rc_prefix}' $? '{rc_suffix}'\n",
            rc_prefix = rc_prefix,
            rc_suffix = rc_suffix,
        );
        let s = self.stdin.as_mut().ok_or("Shell stdin closed")?;
        s.write_all(script.as_bytes())
            .await
            .map_err(|e| format!("Failed to write command: {e}"))?;
        s.flush()
            .await
            .map_err(|e| format!("Failed to flush command: {e}"))?;

        let result = tokio::time::timeout(COMMAND_TIMEOUT, async {
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
                if let Some(rest) = trimmed.strip_prefix(&rc_prefix) {
                    if let Some(code_str) = rest.strip_suffix(rc_suffix) {
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
                let _ = tx.send(BackendEvent::LogLine(kind, content)).await;
            }
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_elapsed) => {
                self.close().await;
                Err(format!(
                    "Command timed out after {} seconds",
                    COMMAND_TIMEOUT.as_secs()
                ))
            }
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
///
/// Every value is wrapped in single quotes. Embedded single quotes are escaped
/// with the `'\''` idiom. This is unconditionally safe for all POSIX sh
/// implementations and removes the need to maintain a character allow-list.
fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

// ── CommandRunner ────────────────────────────────────────────────────────

/// Runs system commands and streams output back to the UI via an async channel.
#[derive(Clone)]
pub struct CommandRunner {
    tx: async_channel::Sender<BackendEvent>,
    kind: BackendKind,
    shell: Option<Arc<Mutex<PrivilegedShell>>>,
}

impl CommandRunner {
    pub fn new(
        tx: async_channel::Sender<BackendEvent>,
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
                    let _ = tx_stdout
                        .send(BackendEvent::LogLine(kind_stdout, line))
                        .await;
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
                    let _ = tx_stderr
                        .send(BackendEvent::LogLine(kind_stderr, line))
                        .await;
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
        let _ = self.tx.send(BackendEvent::LogLine(self.kind, msg)).await;
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
