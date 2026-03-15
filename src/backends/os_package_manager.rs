use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::runner::CommandRunner;
use std::sync::Arc;

/// Detect the OS package manager.
pub fn detect() -> Option<Arc<dyn Backend>> {
    if which::which("apt").is_ok() {
        Some(Arc::new(AptBackend))
    } else if which::which("dnf").is_ok() {
        Some(Arc::new(DnfBackend))
    } else if which::which("pacman").is_ok() {
        Some(Arc::new(PacmanBackend))
    } else if which::which("zypper").is_ok() {
        Some(Arc::new(ZypperBackend))
    } else {
        None
    }
}

// --- APT ---
pub struct AptBackend;

#[async_trait::async_trait]
impl Backend for AptBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Apt
    }
    fn display_name(&self) -> &str {
        "APT"
    }
    fn description(&self) -> &str {
        "Debian / Ubuntu packages"
    }
    fn icon_name(&self) -> &str {
        "system-software-install-symbolic"
    }

    async fn run_update(&self, runner: &CommandRunner) -> UpdateResult {
        if let Err(e) = runner.run("pkexec", &["apt", "update"]).await {
            return UpdateResult::Error(e);
        }
        match runner.run("pkexec", &["apt", "upgrade", "-y"]).await {
            Ok(output) => {
                let count = count_apt_upgraded(&output);
                UpdateResult::Success {
                    updated_count: count,
                }
            }
            Err(e) => UpdateResult::Error(e),
        }
    }

    async fn count_available(&self) -> Result<usize, String> {
        let out = tokio::process::Command::new("apt")
            .args(["list", "--upgradable"])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(text.lines().filter(|l| l.contains('/')).count())
    }
}

fn count_apt_upgraded(output: &str) -> usize {
    // apt upgrade output: "N upgraded, ..."
    for line in output.lines() {
        if line.contains("upgraded") {
            if let Some(n) = line.split_whitespace().next() {
                if let Ok(count) = n.parse::<usize>() {
                    return count;
                }
            }
        }
    }
    0
}

// --- DNF ---
pub struct DnfBackend;

#[async_trait::async_trait]
impl Backend for DnfBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Dnf
    }
    fn display_name(&self) -> &str {
        "DNF"
    }
    fn description(&self) -> &str {
        "Fedora / RHEL packages"
    }
    fn icon_name(&self) -> &str {
        "system-software-install-symbolic"
    }

    async fn run_update(&self, runner: &CommandRunner) -> UpdateResult {
        match runner.run("pkexec", &["dnf", "upgrade", "-y"]).await {
            Ok(output) => {
                let count = count_dnf_upgraded(&output);
                UpdateResult::Success {
                    updated_count: count,
                }
            }
            Err(e) => UpdateResult::Error(e),
        }
    }

    async fn count_available(&self) -> Result<usize, String> {
        let out = tokio::process::Command::new("dnf")
            .args(["check-update"])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        if out.status.code() == Some(0) {
            return Ok(0);
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let count = text
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with("Last") && !l.starts_with("Obsoleting"))
            .count();
        Ok(count)
    }
}

fn count_dnf_upgraded(output: &str) -> usize {
    // Look for "Upgraded:" section
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Upgraded") || trimmed.starts_with("Installed") {
            // e.g., "Upgraded  15 Packages"
            for word in trimmed.split_whitespace() {
                if let Ok(n) = word.parse::<usize>() {
                    return n;
                }
            }
        }
    }
    0
}

// --- Pacman ---
pub struct PacmanBackend;

#[async_trait::async_trait]
impl Backend for PacmanBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Pacman
    }
    fn display_name(&self) -> &str {
        "Pacman"
    }
    fn description(&self) -> &str {
        "Arch Linux packages"
    }
    fn icon_name(&self) -> &str {
        "system-software-install-symbolic"
    }

    async fn run_update(&self, runner: &CommandRunner) -> UpdateResult {
        match runner
            .run("pkexec", &["pacman", "-Syu", "--noconfirm"])
            .await
        {
            Ok(output) => {
                let count = output
                    .lines()
                    .filter(|l| l.starts_with("upgrading ") || l.starts_with("installing "))
                    .count();
                UpdateResult::Success {
                    updated_count: count,
                }
            }
            Err(e) => UpdateResult::Error(e),
        }
    }

    async fn count_available(&self) -> Result<usize, String> {
        let out = tokio::process::Command::new("pacman")
            .args(["-Qu"])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(text.lines().filter(|l| !l.is_empty()).count())
    }
}

// --- Zypper ---
pub struct ZypperBackend;

#[async_trait::async_trait]
impl Backend for ZypperBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Zypper
    }
    fn display_name(&self) -> &str {
        "Zypper"
    }
    fn description(&self) -> &str {
        "openSUSE packages"
    }
    fn icon_name(&self) -> &str {
        "system-software-install-symbolic"
    }

    async fn run_update(&self, runner: &CommandRunner) -> UpdateResult {
        if let Err(e) = runner.run("pkexec", &["zypper", "refresh"]).await {
            return UpdateResult::Error(e);
        }
        match runner.run("pkexec", &["zypper", "update", "-y"]).await {
            Ok(output) => {
                let count = output.lines().filter(|l| l.contains("done")).count();
                UpdateResult::Success {
                    updated_count: count,
                }
            }
            Err(e) => UpdateResult::Error(e),
        }
    }

    async fn count_available(&self) -> Result<usize, String> {
        let out = tokio::process::Command::new("zypper")
            .args(["list-updates"])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(text.lines().filter(|l| l.starts_with("v ")).count())
    }
}
