use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::runner::CommandRunner;
use std::future::Future;
use std::pin::Pin;
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

    fn run_update<'a>(
        &'a self,
        runner: &'a CommandRunner,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            if let Err(e) = runner
                .run(
                    "pkexec",
                    &["env", "DEBIAN_FRONTEND=noninteractive", "apt", "update"],
                )
                .await
            {
                return UpdateResult::Error(e);
            }
            match runner
                .run(
                    "pkexec",
                    &["env", "DEBIAN_FRONTEND=noninteractive", "apt", "upgrade", "-y"],
                )
                .await
            {
                Ok(output) => {
                    let count = count_apt_upgraded(&output);
                    UpdateResult::Success {
                        updated_count: count,
                    }
                }
                Err(e) => UpdateResult::Error(e),
            }
        })
    }

    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("apt")
                .args(["list", "--upgradable"])
                .output()
                .await
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            Ok(text.lines().filter(|l| l.contains('/')).count())
        })
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

    fn run_update<'a>(
        &'a self,
        runner: &'a CommandRunner,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            match runner
                .run("pkexec", &["dnf", "upgrade", "-y"])
                .await
            {
                Ok(output) => {
                    let count = count_dnf_upgraded(&output);
                    UpdateResult::Success {
                        updated_count: count,
                    }
                }
                Err(e) => UpdateResult::Error(e),
            }
        })
    }

    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async move {
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
        })
    }
}

fn count_dnf_upgraded(output: &str) -> usize {
    for line in output.lines() {
        let trimmed = line.trim();
        // DNF4 Transaction Summary: "  Upgrade  15 Packages"
        // DNF5 Transaction Summary: "  Upgrading: 15 packages"
        if trimmed.starts_with("Upgrade ") || trimmed.starts_with("Upgrading:") {
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

    fn run_update<'a>(
        &'a self,
        runner: &'a CommandRunner,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
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
        })
    }

    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("pacman")
                .args(["-Qu"])
                .output()
                .await
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            Ok(text.lines().filter(|l| !l.is_empty()).count())
        })
    }
}

// --- Zypper ---
pub struct ZypperBackend;

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

    fn run_update<'a>(
        &'a self,
        runner: &'a CommandRunner,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
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
        })
    }

    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("zypper")
                .args(["list-updates"])
                .output()
                .await
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            Ok(text.lines().filter(|l| l.starts_with("v ")).count())
        })
    }
}
