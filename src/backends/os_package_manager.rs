use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::executor::CommandExecutor;
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
    fn needs_root(&self) -> bool {
        true
    }

    fn run_update<'a>(
        &'a self,
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            // Single pkexec invocation so polkit only prompts once.
            match runner
                .run(
                    "pkexec",
                    &[
                        "sh",
                        "-c",
                        "DEBIAN_FRONTEND=noninteractive apt update && \
                         DEBIAN_FRONTEND=noninteractive apt upgrade -y",
                    ],
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

    fn list_available(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("apt")
                .args(["list", "--upgradable"])
                .output()
                .await
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            Ok(parse_apt_list_upgradable(&text))
        })
    }
}

pub(crate) fn parse_apt_list_upgradable(output: &str) -> Vec<String> {
    // Lines like: "htop/noble,now 3.3.0 amd64 [upgradable from: 3.2.2]"
    // Skip the "Listing..." header and any line without a '/'.
    output
        .lines()
        .filter(|l| l.contains('/'))
        .filter_map(|l| l.split('/').next().map(|s| s.to_string()))
        .collect()
}

pub(crate) fn count_apt_upgraded(output: &str) -> usize {
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
    fn needs_root(&self) -> bool {
        true
    }

    fn run_update<'a>(
        &'a self,
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            match runner.run("pkexec", &["dnf", "upgrade", "-y"]).await {
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

    fn list_available(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("dnf")
                .args(["check-update"])
                .output()
                .await
                .map_err(|e| e.to_string())?;
            // Exit code 1 = error; 0 = up to date; 100 = updates available
            if out.status.code() == Some(1) {
                return Err("dnf check-update failed".to_string());
            }
            let text = String::from_utf8_lossy(&out.stdout);
            Ok(parse_dnf_list_upgrades(&text))
        })
    }
}

pub(crate) fn parse_dnf_list_upgrades(output: &str) -> Vec<String> {
    // Lines from `dnf check-update`: skip metadata headers, extract package name (first field).
    output
        .lines()
        .filter(|l| {
            !l.is_empty()
                && !l.starts_with("Last")
                && !l.starts_with("Obsoleting")
                && !l.starts_with("Security")
        })
        .filter_map(|l| l.split_whitespace().next().map(|s| s.to_string()))
        .collect()
}

pub(crate) fn count_dnf_upgraded(output: &str) -> usize {
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
    fn needs_root(&self) -> bool {
        true
    }

    fn run_update<'a>(
        &'a self,
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            match runner
                .run("pkexec", &["pacman", "-Syu", "--noconfirm"])
                .await
            {
                Ok(output) => {
                    let count = count_pacman_upgraded(&output);
                    UpdateResult::Success {
                        updated_count: count,
                    }
                }
                Err(e) => UpdateResult::Error(e),
            }
        })
    }

    fn list_available(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("pacman")
                .args(["-Qu"])
                .output()
                .await
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            Ok(parse_checkupdates(&text))
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
    fn needs_root(&self) -> bool {
        true
    }

    fn run_update<'a>(
        &'a self,
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            // Single pkexec invocation so polkit only prompts once.
            match runner
                .run(
                    "pkexec",
                    &["sh", "-c", "zypper refresh && zypper update -y"],
                )
                .await
            {
                Ok(output) => {
                    let count = count_zypper_upgraded(&output);
                    UpdateResult::Success {
                        updated_count: count,
                    }
                }
                Err(e) => UpdateResult::Error(e),
            }
        })
    }

    fn list_available(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("zypper")
                .args(["list-updates"])
                .output()
                .await
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            Ok(parse_zypper_list_updates(&text))
        })
    }
}

pub(crate) fn parse_checkupdates(output: &str) -> Vec<String> {
    // Lines from `pacman -Qu`: "pkgname old-ver -> new-ver" — extract package name.
    output
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|l| l.split_whitespace().next().map(|s| s.to_string()))
        .collect()
}

pub(crate) fn count_pacman_upgraded(output: &str) -> usize {
    output
        .lines()
        .filter(|l| l.starts_with("upgrading ") || l.starts_with("installing "))
        .count()
}

pub(crate) fn parse_zypper_list_updates(output: &str) -> Vec<String> {
    // Table rows from `zypper list-updates` starting with "v " —
    // extract 3rd pipe-delimited column (package name).
    output
        .lines()
        .filter(|l| l.starts_with("v "))
        .filter_map(|l| l.split('|').nth(2).map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect()
}

pub(crate) fn count_zypper_upgraded(output: &str) -> usize {
    output.lines().filter(|l| l.contains("done")).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::BackendError;
    use crate::executor::test_utils::MockExecutor;

    #[test]
    fn test_count_apt_upgraded_zero() {
        let output = "0 upgraded, 0 newly installed, 0 to remove and 0 not upgraded.";
        assert_eq!(count_apt_upgraded(output), 0);
    }

    #[test]
    fn test_count_apt_upgraded_some() {
        let output = "3 upgraded, 0 newly installed, 0 to remove and 1 not upgraded.";
        assert_eq!(count_apt_upgraded(output), 3);
    }

    #[test]
    fn test_count_apt_upgraded_no_match() {
        let output = "Reading package lists...\nBuilding dependency tree...";
        assert_eq!(count_apt_upgraded(output), 0);
    }

    #[test]
    fn test_count_dnf_upgraded_dnf4() {
        let output = "  Upgrade  15 Packages\n\nTransaction Summary";
        assert_eq!(count_dnf_upgraded(output), 15);
    }

    #[test]
    fn test_count_dnf_upgraded_dnf5() {
        let output = "  Upgrading: 7 packages";
        assert_eq!(count_dnf_upgraded(output), 7);
    }

    #[test]
    fn test_count_dnf_upgraded_none() {
        let output = "Nothing to do.\nComplete!";
        assert_eq!(count_dnf_upgraded(output), 0);
    }

    #[test]
    fn test_count_pacman_upgraded_some() {
        let output = "resolving dependencies...\nupgrading htop\nupgrading curl\ninstalling dep\n";
        assert_eq!(count_pacman_upgraded(output), 3);
    }

    #[test]
    fn test_count_pacman_upgraded_none() {
        let output = "there is nothing to do\n";
        assert_eq!(count_pacman_upgraded(output), 0);
    }

    #[test]
    fn test_count_zypper_upgraded_some() {
        let output =
            "Retrieving package htop.rpm (1/2)...done\nRetrieving package curl.rpm (2/2)...done\n";
        assert_eq!(count_zypper_upgraded(output), 2);
    }

    #[test]
    fn test_count_zypper_upgraded_none() {
        let output = "Nothing to do.\n";
        assert_eq!(count_zypper_upgraded(output), 0);
    }

    #[test]
    fn test_parse_apt_list_upgradable_happy_path() {
        let output = "Listing... Done\nhtop/noble 3.3.0-1 amd64 [upgradable from: 3.2.2-1]\ncurl/noble 8.5.0-1 amd64 [upgradable from: 8.4.0-1]\n";
        let result = parse_apt_list_upgradable(output);
        assert_eq!(result, vec!["htop".to_string(), "curl".to_string()]);
    }

    #[test]
    fn test_parse_apt_list_upgradable_only_header() {
        assert!(parse_apt_list_upgradable("Listing... Done\n").is_empty());
    }

    #[test]
    fn test_parse_dnf_list_upgrades_happy_path() {
        let output = "Last metadata expiration check: 0:01:23 ago.\nhtop.x86_64   3.3.0-2.fc40  updates\ncurl.x86_64   8.5.0-1.fc40  updates\n";
        let result = parse_dnf_list_upgrades(output);
        assert_eq!(
            result,
            vec!["htop.x86_64".to_string(), "curl.x86_64".to_string()]
        );
    }

    #[test]
    fn test_parse_dnf_list_upgrades_empty() {
        assert!(
            parse_dnf_list_upgrades("Last metadata expiration check: 0:01:23 ago.\n").is_empty()
        );
    }

    #[test]
    fn test_parse_checkupdates_happy_path() {
        let output = "htop 3.2.2-1 -> 3.3.0-1\ncurl 8.4.0-1 -> 8.5.0-1\n";
        let result = parse_checkupdates(output);
        assert_eq!(result, vec!["htop".to_string(), "curl".to_string()]);
    }

    #[test]
    fn test_parse_checkupdates_empty() {
        assert!(parse_checkupdates("").is_empty());
    }

    #[test]
    fn test_parse_zypper_list_updates_happy_path() {
        let output = "v | openSUSE-updates | htop | 3.2.2-1.1 | 3.3.0-1.1 | x86_64\nv | openSUSE-updates | curl | 8.4.0-1.1 | 8.5.0-1.1 | x86_64\n";
        let result = parse_zypper_list_updates(output);
        assert_eq!(result, vec!["htop".to_string(), "curl".to_string()]);
    }

    #[test]
    fn test_parse_zypper_list_updates_no_updates() {
        assert!(
            parse_zypper_list_updates("Loading repository data...\nNo updates found.\n").is_empty()
        );
    }

    // --- run_update pipeline tests ---

    #[tokio::test]
    async fn test_apt_run_update_success() {
        let output = "Reading package lists...\nBuilding dependency tree...\n3 upgraded, 0 newly installed, 0 to remove and 1 not upgraded.\n";
        let mock = MockExecutor::with_output(output);
        let result = AptBackend.run_update(&mock).await;
        assert!(
            matches!(result, UpdateResult::Success { updated_count: 3 }),
            "Expected Success {{ updated_count: 3 }}, got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_apt_run_update_auth_cancelled() {
        let mock = MockExecutor::new(vec![Err(BackendError::AuthCancelled)]);
        let result = AptBackend.run_update(&mock).await;
        assert!(
            matches!(result, UpdateResult::Error(BackendError::AuthCancelled)),
            "Expected Error(AuthCancelled), got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_dnf_run_update_success() {
        let output = "Last metadata expiration check: 0:00:01 ago.\nDependencies resolved.\n  Upgrading: 7 packages\nComplete!\n";
        let mock = MockExecutor::with_output(output);
        let result = DnfBackend.run_update(&mock).await;
        assert!(
            matches!(result, UpdateResult::Success { updated_count: 7 }),
            "Expected Success {{ updated_count: 7 }}, got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_dnf_run_update_error() {
        let mock = MockExecutor::with_error(1, "dnf upgrade failed");
        let result = DnfBackend.run_update(&mock).await;
        assert!(matches!(result, UpdateResult::Error(_)));
    }

    #[tokio::test]
    async fn test_pacman_run_update_success() {
        let output = "resolving dependencies...\nupgrading htop\nupgrading curl\nupgrading linux\n:: Running post-transaction hooks...\n";
        let mock = MockExecutor::with_output(output);
        let result = PacmanBackend.run_update(&mock).await;
        assert!(
            matches!(result, UpdateResult::Success { updated_count: 3 }),
            "Expected Success {{ updated_count: 3 }}, got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_pacman_run_update_auth_cancelled() {
        let mock = MockExecutor::new(vec![Err(BackendError::AuthCancelled)]);
        let result = PacmanBackend.run_update(&mock).await;
        assert!(
            matches!(result, UpdateResult::Error(BackendError::AuthCancelled)),
            "Expected Error(AuthCancelled), got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_zypper_run_update_success() {
        let output = "Retrieving package htop-3.3.0.x86_64 (1/3)...done\nRetrieving package curl-8.5.0.x86_64 (2/3)...done\nRetrieving package openssl-3.1.4.x86_64 (3/3)...done\n";
        let mock = MockExecutor::with_output(output);
        let result = ZypperBackend.run_update(&mock).await;
        assert!(
            matches!(result, UpdateResult::Success { updated_count: 3 }),
            "Expected Success {{ updated_count: 3 }}, got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_zypper_run_update_error() {
        let mock = MockExecutor::with_error(1, "zypper update failed");
        let result = ZypperBackend.run_update(&mock).await;
        assert!(matches!(result, UpdateResult::Error(_)));
    }
}
