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
    Fut: Future<Output = ()>,
{
    std::thread::spawn(move || {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => {
                rt.block_on(f());
            }
            Err(e) => {
                eprintln!("Failed to build Tokio runtime: {e}");
            }
        }
    });
}
