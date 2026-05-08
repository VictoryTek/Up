pub mod flatpak;
pub mod fwupd;
pub mod homebrew;
pub mod nix;
pub mod os_package_manager;

use crate::executor::CommandExecutor;
use log::info;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Structured error type for backend operations.
#[derive(Debug, thiserror::Error, Clone)]
pub enum BackendError {
    /// pkexec exited with code 126 (auth cancelled) or 127 (not authorised).
    #[error("Authentication cancelled or denied")]
    AuthCancelled,
    /// The command could not be spawned (binary not found, permission error).
    #[error("Failed to spawn process: {0}")]
    Spawn(String),
    /// The command was spawned but exited with a non-zero status code.
    #[error("Command failed (exit {code}): {message}")]
    Exit { code: i32, message: String },
    /// Output from the command could not be parsed.
    #[error("Failed to parse command output: {0}")]
    #[allow(dead_code)]
    Parse(String),
    /// A network operation failed.
    #[error("Network error: {0}")]
    #[allow(dead_code)]
    Network(String),
    /// The update was cancelled by the user.
    #[error("Update cancelled by user")]
    #[allow(dead_code)]
    Cancelled,
}

impl BackendError {
    /// Convert a raw error string into the most specific BackendError variant.
    /// Used as a bridge during migration from String-based errors.
    pub fn from_string(s: String) -> Self {
        let lower = s.to_ascii_lowercase();
        if lower.contains("authentication was cancelled")
            || lower.contains("not authorised")
            || s.contains("exit code 126")
            || s.contains("exit code 127")
        {
            return BackendError::AuthCancelled;
        }
        if lower.contains("failed to start") || lower.contains("no such file or directory") {
            return BackendError::Spawn(s);
        }
        if lower.contains("exited with code") {
            let code = s
                .split("code ")
                .nth(1)
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|n| n.parse::<i32>().ok())
                .unwrap_or(-1);
            return BackendError::Exit { code, message: s };
        }
        BackendError::Exit {
            code: -1,
            message: s,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendKind {
    Apt,
    Dnf,
    Pacman,
    Zypper,
    Flatpak,
    Homebrew,
    Nix,
    Fwupd,
}

impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Apt => write!(f, "APT"),
            Self::Dnf => write!(f, "DNF"),
            Self::Pacman => write!(f, "Pacman"),
            Self::Zypper => write!(f, "Zypper"),
            Self::Flatpak => write!(f, "Flatpak"),
            Self::Homebrew => write!(f, "Homebrew"),
            Self::Nix => write!(f, "Nix"),
            Self::Fwupd => write!(f, "Fwupd"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum UpdateResult {
    Success {
        updated_count: usize,
    },
    /// Emitted by `FlatpakBackend` when running inside the Flatpak sandbox and
    /// the update output indicates that Up itself (`APP_ID`) was updated.  The
    /// UI layer uses this variant to reveal a restart notification banner.
    SuccessWithSelfUpdate {
        updated_count: usize,
    },
    Error(BackendError),
    #[allow(dead_code)]
    Skipped(String),
    /// The update was cancelled by the user before or during execution.
    Cancelled,
}

pub trait Backend: Send + Sync {
    fn kind(&self) -> BackendKind;
    fn display_name(&self) -> &str;
    fn description(&self) -> &str;
    fn icon_name(&self) -> &str;

    fn run_update<'a>(
        &'a self,
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>>;

    /// Whether this backend requires root privileges (pkexec) to perform updates.
    /// Used by the UI to determine if pre-authentication is needed before starting.
    /// Default: false (no root required).
    fn needs_root(&self) -> bool {
        false
    }

    /// Count packages available for update (read-only, no privilege required).
    /// Returns Ok(0) if up to date, Ok(N) if N updates available, Err(_) on failure.
    /// Default implementation delegates to list_available to avoid duplicating command logic.
    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async move { self.list_available().await.map(|v| v.len()) })
    }

    /// Return a human-readable list of package names pending update.
    /// Each element is a short package identifier (e.g., "htop").
    /// Returns Ok(vec![]) for backends that cannot enumerate packages without
    /// performing the update (e.g., NixOS).
    /// Default implementation returns Ok(vec![]) for backward compatibility.
    fn list_available(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// Whether this backend supports a cleanup / maintenance operation.
    /// Default: false. Override to true in backends that implement run_cleanup.
    fn supports_cleanup(&self) -> bool {
        false
    }

    /// Run the cleanup/maintenance operation for this backend, streaming output
    /// through `runner`. Returns UpdateResult where `updated_count` is the number
    /// of packages removed (0 = already clean).
    /// Default: no-op, returns Success { updated_count: 0 }.
    fn run_cleanup<'a>(
        &'a self,
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        let _ = runner;
        Box::pin(async { UpdateResult::Success { updated_count: 0 } })
    }
}

/// Detect all available backends on the current system.
pub fn detect_backends() -> Vec<Arc<dyn Backend>> {
    let mut backends: Vec<Arc<dyn Backend>> = Vec::new();

    // Detect OS package manager
    if let Some(os_backend) = os_package_manager::detect() {
        backends.push(os_backend);
    }

    // Nix — placed before Flatpak so that row order matches execution order
    // (Nix runs privileged and is sorted ahead of unprivileged backends).
    if nix::is_available() {
        backends.push(Arc::new(nix::NixBackend));
    }

    // Flatpak — always include when running inside the Flatpak sandbox so that
    // `flatpak-spawn --host` can be used to update host Flatpak packages even
    // though the `flatpak` binary itself is not on the sandbox PATH.
    if flatpak::is_available() || flatpak::is_running_in_flatpak() {
        backends.push(Arc::new(flatpak::FlatpakBackend));
    }

    // Homebrew
    if homebrew::is_available() {
        backends.push(Arc::new(homebrew::HomebrewBackend));
    }

    // fwupd — firmware updates via LVFS; unprivileged (polkit handled by daemon)
    if fwupd::is_available() {
        backends.push(Arc::new(fwupd::FwupdBackend));
    }

    for b in &backends {
        info!("Backend detected: {}", b.display_name());
    }

    backends
}
