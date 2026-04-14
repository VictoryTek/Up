pub mod flatpak;
pub mod homebrew;
pub mod nix;
pub mod os_package_manager;

use crate::runner::CommandRunner;
use log::info;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendKind {
    Apt,
    Dnf,
    Pacman,
    Zypper,
    Flatpak,
    Homebrew,
    Nix,
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
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    Error(String),
    Skipped(String),
}

pub trait Backend: Send + Sync {
    fn kind(&self) -> BackendKind;
    fn display_name(&self) -> &str;
    fn description(&self) -> &str;
    fn icon_name(&self) -> &str;

    fn run_update<'a>(
        &'a self,
        runner: &'a CommandRunner,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>>;

    /// Count packages available for update (read-only, no privilege required).
    /// Returns Ok(0) if up to date, Ok(N) if N updates available, Err(_) on failure.
    /// Default implementation returns Ok(0) for backends that do not support checking.
    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async { Ok(0) })
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
}

/// Detect all available backends on the current system.
pub fn detect_backends() -> Vec<Arc<dyn Backend>> {
    let mut backends: Vec<Arc<dyn Backend>> = Vec::new();

    // Detect OS package manager
    if let Some(os_backend) = os_package_manager::detect() {
        backends.push(os_backend);
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

    // Nix
    if nix::is_available() {
        backends.push(Arc::new(nix::NixBackend));
    }

    for b in &backends {
        info!("Backend detected: {}", b.display_name());
    }

    backends
}
