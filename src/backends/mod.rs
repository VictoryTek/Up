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
    Success { updated_count: usize },
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
}

/// Detect all available backends on the current system.
pub fn detect_backends() -> Vec<Arc<dyn Backend>> {
    let mut backends: Vec<Arc<dyn Backend>> = Vec::new();

    // Detect OS package manager
    if let Some(os_backend) = os_package_manager::detect() {
        backends.push(os_backend);
    }

    // Flatpak
    if flatpak::is_available() {
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
