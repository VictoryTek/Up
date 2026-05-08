pub mod history_page;
pub mod log_panel;
pub mod reboot_dialog;
pub mod update_row;
pub mod upgrade_page;
pub mod window;

use std::future::Future;

/// Spawns a background OS thread, creates a single-threaded Tokio runtime on
/// that thread, and drives the provided async closure to completion.
///
/// This avoids repeating the `std::thread::spawn` + `tokio::runtime::Builder`
/// boilerplate at every call site. If the Tokio runtime fails to build (which
/// is extremely rare in practice), the error is logged to stderr.
pub(crate) fn spawn_background_async<F, Fut>(f: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    drop(crate::runtime::runtime().spawn(f()));
}
