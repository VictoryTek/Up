#![allow(dead_code)]
use crate::backends;
use crate::runtime::runtime;
use log::{info, warn};
use std::path::{Path, PathBuf};

/// Entry point for `up --check`.
///
/// Detects all available backends, counts pending updates, compares against
/// the previous run's stamp file, and fires a desktop notification if the
/// count has changed and is non-zero.
///
/// Runs synchronously from `main()` before any GTK initialisation.
pub fn run_check() {
    env_logger::init();

    let backends = backends::detect_backends();

    if backends.is_empty() {
        info!("up --check: no backends detected, exiting");
        return;
    }

    let total: usize = runtime().block_on(async {
        let mut sum = 0usize;
        for backend in &backends {
            match backend.count_available().await {
                Ok(n) => {
                    info!(
                        "up --check: {} reports {} update(s)",
                        backend.display_name(),
                        n
                    );
                    sum += n;
                }
                Err(e) => {
                    warn!("up --check: {} error: {}", backend.display_name(), e);
                }
            }
        }
        sum
    });

    info!("up --check: total updates available = {}", total);

    let stamp_path = stamp_file_path();
    let prev_count = read_stamp(&stamp_path);

    if total > 0 && Some(total) != prev_count {
        send_notification(total);
    }

    // Always update stamp so the next run has an accurate baseline.
    write_stamp(&stamp_path, total);
}

// ── Stamp file ────────────────────────────────────────────────────────────────

/// Returns `$XDG_CACHE_HOME/up/last-check-count` (fallback: `$HOME/.cache/up/…`).
fn stamp_file_path() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))
                .join(".cache")
        });
    base.join("up").join("last-check-count")
}

/// Returns `Some(n)` if a valid stamp exists, `None` otherwise.
fn read_stamp(path: &Path) -> Option<usize> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Writes `count` to the stamp file, creating parent directories as needed.
fn write_stamp(path: &Path, count: usize) {
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            warn!(
                "up --check: could not create cache dir {}: {}",
                parent.display(),
                e
            );
            return;
        }
    }
    if let Err(e) = std::fs::write(path, count.to_string()) {
        warn!(
            "up --check: could not write stamp file {}: {}",
            path.display(),
            e
        );
    }
}

// ── Notification ──────────────────────────────────────────────────────────────

fn send_notification(count: usize) {
    let summary = if count == 1 {
        "1 update available".to_string()
    } else {
        format!("{} updates available", count)
    };
    let body = "Open Up to review and apply updates.";

    let status = std::process::Command::new("notify-send")
        .args([
            "-a",
            "Up",
            "-i",
            "io.github.up",
            "-u",
            "normal",
            &summary,
            body,
        ])
        .status();

    match status {
        Ok(s) if s.success() => info!("up --check: notification sent ({} updates)", count),
        Ok(s) => warn!("up --check: notify-send exited with status {}", s),
        Err(e) => warn!("up --check: could not spawn notify-send: {}", e),
    }
}
