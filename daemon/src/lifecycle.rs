use log::info;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Tracks daemon activity for idle timeout management.
pub struct IdleTracker {
    last_activity: Instant,
    timeout: Duration,
}

impl IdleTracker {
    pub fn new(timeout: Duration) -> Self {
        Self {
            last_activity: Instant::now(),
            timeout,
        }
    }

    /// Mark that activity has occurred, resetting the idle timer.
    pub fn mark_active(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Check if the daemon has been idle longer than the configured timeout.
    pub fn is_idle(&self) -> bool {
        self.last_activity.elapsed() >= self.timeout
    }

    /// Get remaining time before idle timeout.
    #[allow(dead_code)]
    pub fn remaining(&self) -> Duration {
        let elapsed = self.last_activity.elapsed();
        if elapsed >= self.timeout {
            Duration::ZERO
        } else {
            self.timeout - elapsed
        }
    }
}

/// Wait for either idle timeout or shutdown signal.
/// Polls the idle tracker periodically and exits when the daemon should shut down.
pub async fn wait_for_shutdown(
    idle_tracker: Arc<Mutex<IdleTracker>>,
    _connection: zbus::Connection,
) {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;

        let tracker = idle_tracker.lock().await;
        if tracker.is_idle() {
            info!("Idle timeout reached, initiating shutdown");
            return;
        }
    }
}
