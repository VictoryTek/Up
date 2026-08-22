use crate::backends::BackendError;
use std::future::Future;
use std::pin::Pin;

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
}

#[cfg(test)]
pub mod test_utils {
    use super::*;
    use crate::backends::BackendError;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// A test double for [`CommandExecutor`] that returns pre-configured responses
    /// in FIFO order. Each call to `run` consumes one response from the queue.
    ///
    /// Panics if `run` is called more times than responses were enqueued.
    #[derive(Clone)]
    pub struct MockExecutor {
        responses: Arc<Mutex<VecDeque<Result<String, BackendError>>>>,
        calls: Arc<Mutex<Vec<(String, Vec<String>)>>>,
    }

    impl MockExecutor {
        /// Create a `MockExecutor` pre-loaded with the given responses.
        /// The first call to `run` returns `responses[0]`, the second returns `responses[1]`, etc.
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
    }

    impl CommandExecutor for MockExecutor {
        fn run<'a>(
            &'a self,
            program: &'a str,
            args: &'a [&'a str],
        ) -> Pin<Box<dyn Future<Output = Result<String, BackendError>> + Send + 'a>> {
            self.calls
                .lock()
                .expect("MockExecutor mutex poisoned")
                .push((
                    program.to_string(),
                    args.iter().map(|s| s.to_string()).collect(),
                ));
            let response = self
                .responses
                .lock()
                .expect("MockExecutor mutex poisoned")
                .pop_front()
                .expect(
                    "MockExecutor: no more pre-configured responses (run() called too many times)",
                );
            Box::pin(async move { response })
        }
    }
}
