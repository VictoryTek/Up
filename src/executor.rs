use crate::backends::BackendError;
use std::future::Future;
use std::pin::Pin;

/// Result of a read-only probe command run via [`CommandExecutor::probe`].
///
/// Unlike [`CommandExecutor::run`], a probe never treats a non-zero exit as an
/// error: the caller inspects `code` / `stdout` / `stderr` directly. `spawned`
/// is `false` only when the process could not be started at all.
#[derive(Debug, Clone)]
pub struct ProbeOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: Option<i32>,
    pub spawned: bool,
}

impl ProbeOutput {
    /// True when the process spawned and exited 0.
    pub fn ok(&self) -> bool {
        self.spawned && self.code == Some(0)
    }
}

/// Abstracts the execution of external system commands, enabling dependency injection
/// and test doubles.
///
/// Implementations must be `Send + Sync` so they can be shared across async boundaries.
pub trait CommandExecutor: Send + Sync {
    /// Run `program` with `args`, stream output line-by-line internally,
    /// and return the full combined output on success.
    ///
    /// Returns `Err(BackendError)` on non-zero exit, spawn failure, or auth cancellation.
    fn run<'a>(
        &'a self,
        program: &'a str,
        args: &'a [&'a str],
    ) -> Pin<Box<dyn Future<Output = Result<String, BackendError>> + Send + 'a>>;

    /// Run a read-only probe command: capture stdout + stderr, never treat a
    /// non-zero exit as an error, and do not stream output to the log panel.
    ///
    /// `env` entries are applied to the child process environment.
    fn probe<'a>(
        &'a self,
        program: &'a str,
        args: &'a [&'a str],
        env: &'a [(&'a str, &'a str)],
    ) -> Pin<Box<dyn Future<Output = ProbeOutput> + Send + 'a>>;
}

/// Spawn a command, capture its full output, and map the result to a
/// [`ProbeOutput`]. Shared by [`SystemExecutor`] and `CommandRunner` so their
/// probe behaviour stays identical.
pub(crate) async fn spawn_probe(program: &str, args: &[&str], env: &[(&str, &str)]) -> ProbeOutput {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    match cmd.output().await {
        Ok(out) => ProbeOutput {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            code: out.status.code(),
            spawned: true,
        },
        Err(e) => ProbeOutput {
            stdout: String::new(),
            stderr: e.to_string(),
            code: None,
            spawned: false,
        },
    }
}

/// A [`CommandExecutor`] that runs commands directly with no log-panel
/// streaming. Used for read-only probes at call sites that have no
/// `BackendEvent` channel (the CLI `--check` path and the UI check cycle).
pub struct SystemExecutor;

impl CommandExecutor for SystemExecutor {
    fn run<'a>(
        &'a self,
        program: &'a str,
        args: &'a [&'a str],
    ) -> Pin<Box<dyn Future<Output = Result<String, BackendError>> + Send + 'a>> {
        Box::pin(async move {
            let out = spawn_probe(program, args, &[]).await;
            if !out.spawned {
                return Err(BackendError::Spawn(format!(
                    "Failed to start {program}: {}",
                    out.stderr
                )));
            }
            if out.code == Some(0) {
                Ok(format!("{}{}", out.stdout, out.stderr))
            } else {
                Err(BackendError::Exit {
                    code: out.code.unwrap_or(-1),
                    message: out.stderr,
                })
            }
        })
    }

    fn probe<'a>(
        &'a self,
        program: &'a str,
        args: &'a [&'a str],
        env: &'a [(&'a str, &'a str)],
    ) -> Pin<Box<dyn Future<Output = ProbeOutput> + Send + 'a>> {
        Box::pin(spawn_probe(program, args, env))
    }
}

#[cfg(test)]
pub mod test_utils {
    use super::*;
    use crate::backends::BackendError;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// A test double for [`CommandExecutor`] that returns pre-configured responses
    /// in FIFO order. Each call to `run` or `probe` consumes one response from the queue.
    ///
    /// Panics if called more times than responses were enqueued.
    #[derive(Clone)]
    pub struct MockExecutor {
        responses: Arc<Mutex<VecDeque<Result<String, BackendError>>>>,
        calls: Arc<Mutex<Vec<(String, Vec<String>)>>>,
    }

    impl MockExecutor {
        /// Create a `MockExecutor` pre-loaded with the given responses.
        /// The first call returns `responses[0]`, the second returns `responses[1]`, etc.
        pub fn new(responses: Vec<Result<String, BackendError>>) -> Self {
            Self {
                responses: Arc::new(Mutex::new(responses.into())),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        /// Returns the `(program, args)` of every call made so far, in order.
        pub fn calls(&self) -> Vec<(String, Vec<String>)> {
            self.calls
                .lock()
                .expect("MockExecutor mutex poisoned")
                .clone()
        }

        /// Convenience: create with a single successful output string.
        pub fn with_output(output: impl Into<String>) -> Self {
            Self::new(vec![Ok(output.into())])
        }

        /// Convenience: create with a single `BackendError::Exit` response.
        pub fn with_error(code: i32, message: impl Into<String>) -> Self {
            Self::new(vec![Err(BackendError::Exit {
                code,
                message: message.into(),
            })])
        }

        /// Convenience: create with a single probe response that carries usable
        /// stdout together with an arbitrary exit code (e.g. dnf exit 100).
        pub fn with_probe(stdout: impl Into<String>, code: i32) -> Self {
            Self::new(vec![Err(BackendError::Exit {
                code,
                message: stdout.into(),
            })])
        }

        fn record(&self, program: &str, args: &[&str]) {
            self.calls
                .lock()
                .expect("MockExecutor mutex poisoned")
                .push((
                    program.to_string(),
                    args.iter().map(|s| s.to_string()).collect(),
                ));
        }

        fn next_response(&self) -> Result<String, BackendError> {
            self.responses
                .lock()
                .expect("MockExecutor mutex poisoned")
                .pop_front()
                .expect("MockExecutor: no more pre-configured responses (called too many times)")
        }
    }

    impl CommandExecutor for MockExecutor {
        fn run<'a>(
            &'a self,
            program: &'a str,
            args: &'a [&'a str],
        ) -> Pin<Box<dyn Future<Output = Result<String, BackendError>> + Send + 'a>> {
            self.record(program, args);
            let response = self.next_response();
            Box::pin(async move { response })
        }

        fn probe<'a>(
            &'a self,
            program: &'a str,
            args: &'a [&'a str],
            _env: &'a [(&'a str, &'a str)],
        ) -> Pin<Box<dyn Future<Output = ProbeOutput> + Send + 'a>> {
            self.record(program, args);
            let out = match self.next_response() {
                Ok(stdout) => ProbeOutput {
                    stdout,
                    stderr: String::new(),
                    code: Some(0),
                    spawned: true,
                },
                Err(BackendError::Exit { code, message }) => ProbeOutput {
                    stdout: message.clone(),
                    stderr: message,
                    code: Some(code),
                    spawned: true,
                },
                Err(BackendError::Spawn(msg)) => ProbeOutput {
                    stdout: String::new(),
                    stderr: msg,
                    code: None,
                    spawned: false,
                },
                Err(other) => ProbeOutput {
                    stdout: String::new(),
                    stderr: other.to_string(),
                    code: Some(-1),
                    spawned: true,
                },
            };
            Box::pin(async move { out })
        }
    }
}
