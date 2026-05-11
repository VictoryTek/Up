mod allowlist;
mod audit;
mod auth;
mod cancel;
mod executor;
mod interface;
mod lifecycle;

use log::info;
use std::sync::Arc;
use tokio::sync::Mutex;

use interface::UpDaemon;
use lifecycle::IdleTracker;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    info!("up-daemon starting (version {})", env!("CARGO_PKG_VERSION"));

    let idle_tracker = Arc::new(Mutex::new(IdleTracker::new(
        std::time::Duration::from_secs(60),
    )));

    let daemon = UpDaemon::new(idle_tracker.clone());

    // Connect to the system bus and serve the interface
    let connection = zbus::connection::Builder::system()?
        .name("io.github.up.Daemon")?
        .serve_at("/io/github/up/Daemon", daemon)?
        .build()
        .await?;

    info!("up-daemon connected to system bus");

    // Run until idle timeout or SIGTERM
    let conn_clone = connection.clone();
    let shutdown = lifecycle::wait_for_shutdown(idle_tracker, conn_clone);

    // Wait for SIGTERM or idle timeout
    tokio::select! {
        _ = shutdown => {
            info!("up-daemon shutting down (idle timeout)");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("up-daemon shutting down (signal)");
        }
    }

    Ok(())
}
