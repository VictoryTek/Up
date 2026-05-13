use crate::backends::{Backend, BackendError, BackendKind, UpdateResult};
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

    fn estimate_size(&self) -> Pin<Box<dyn Future<Output = Option<u64>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("apt-get")
                .args(["-s", "upgrade"])
                .env("LANG", "C")
                .env("LC_ALL", "C")
                .env("DEBIAN_FRONTEND", "noninteractive")
                .output()
                .await
                .ok()?;
            if !out.status.success() {
                return None;
            }
            let text = String::from_utf8_lossy(&out.stdout);
            crate::disk::parse_apt_size(&text)
        })
    }

    fn supports_cleanup(&self) -> bool {
        true
    }

    fn run_cleanup<'a>(
        &'a self,
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            match runner
                .run(
                    "pkexec",
                    &[
                        "sh",
                        "-c",
                        "DEBIAN_FRONTEND=noninteractive apt autoremove -y",
                    ],
                )
                .await
            {
                Ok(output) => {
                    let removed = count_apt_autoremovals(&output);
                    UpdateResult::Success {
                        updated_count: removed,
                    }
                }
                Err(e) => UpdateResult::Error(e),
            }
        })
    }

    fn supports_item_selection(&self) -> bool {
        true
    }

    fn run_selected_update<'a>(
        &'a self,
        items: &'a [String],
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            // Validate: package names must be safe shell tokens.
            for pkg in items {
                if pkg.is_empty()
                    || pkg.len() > 255
                    || pkg.chars().any(|c| {
                        !(c.is_ascii_alphanumeric()
                            || c == '-'
                            || c == '+'
                            || c == '.'
                            || c == '_'
                            || c == ':')
                    })
                {
                    return UpdateResult::Error(BackendError::from_string(format!(
                        "Invalid package name: {:?}",
                        pkg
                    )));
                }
            }
            // DEBIAN_FRONTEND must be set in the shell environment; sh -c is required here
            let pkg_list = items.join(" ");
            let cmd = format!(
                "DEBIAN_FRONTEND=noninteractive apt-get install --only-upgrade -y {}",
                pkg_list
            );
            match runner.run("pkexec", &["sh", "-c", &cmd]).await {
                Ok(output) => UpdateResult::Success {
                    updated_count: count_apt_upgraded(&output),
                },
                Err(e) => UpdateResult::Error(e),
            }
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

    fn estimate_size(&self) -> Pin<Box<dyn Future<Output = Option<u64>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("dnf")
                .args(["upgrade", "--assumeno"])
                .env("LANG", "C")
                .env("LC_ALL", "C")
                .output()
                .await
                .ok()?;
            // DNF exits non-zero when packages are available; stdout still has the summary.
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let combined = format!("{stdout}\n{stderr}");
            crate::disk::parse_dnf_size(&combined)
        })
    }

    fn supports_cleanup(&self) -> bool {
        true
    }

    fn run_cleanup<'a>(
        &'a self,
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            match runner.run("pkexec", &["dnf", "autoremove", "-y"]).await {
                Ok(output) => {
                    let removed = count_dnf_autoremovals(&output);
                    UpdateResult::Success {
                        updated_count: removed,
                    }
                }
                Err(e) => UpdateResult::Error(e),
            }
        })
    }

    fn supports_item_selection(&self) -> bool {
        true
    }

    fn run_selected_update<'a>(
        &'a self,
        items: &'a [String],
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            // Validate: package names must be safe shell tokens.
            for pkg in items {
                if pkg.is_empty()
                    || pkg.len() > 255
                    || pkg
                        .chars()
                        .any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_'))
                {
                    return UpdateResult::Error(BackendError::from_string(format!(
                        "Invalid package name: {:?}",
                        pkg
                    )));
                }
            }
            let mut args = vec!["dnf", "upgrade", "-y"];
            args.extend(items.iter().map(|s| s.as_str()));
            match runner.run("pkexec", &args).await {
                Ok(output) => UpdateResult::Success {
                    updated_count: count_dnf_upgraded(&output),
                },
                Err(e) => UpdateResult::Error(e),
            }
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

    fn supports_cleanup(&self) -> bool {
        true
    }

    fn run_cleanup<'a>(
        &'a self,
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            // Step 1: List orphans unprivileged.
            let qtdq_out = match tokio::process::Command::new("pacman")
                .args(["-Qtdq"])
                .output()
                .await
            {
                Ok(o) => o,
                Err(e) => {
                    return UpdateResult::Error(BackendError::Spawn(e.to_string()));
                }
            };

            // pacman -Qtdq exits non-zero on some versions when there are no orphans;
            // treat any exit as "no orphans" when stdout is empty.
            let stdout = String::from_utf8_lossy(&qtdq_out.stdout);
            let orphans: Vec<String> = stdout
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();

            if orphans.is_empty() {
                return UpdateResult::Success { updated_count: 0 };
            }

            // Step 2: Remove orphans with privilege.
            let mut args: Vec<&str> = vec!["pacman", "-Rns", "--noconfirm"];
            args.extend(orphans.iter().map(|s| s.as_str()));

            match runner.run("pkexec", &args).await {
                Ok(_) => UpdateResult::Success {
                    updated_count: orphans.len(),
                },
                Err(e) => UpdateResult::Error(e),
            }
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
                    &[
                        "sh",
                        "-c",
                        "LANG=C LC_ALL=C zypper refresh && LANG=C LC_ALL=C zypper update -y",
                    ],
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

    fn estimate_size(&self) -> Pin<Box<dyn Future<Output = Option<u64>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("zypper")
                .args(["--non-interactive", "--no-color", "update", "--dry-run"])
                .env("LANG", "C")
                .env("LC_ALL", "C")
                .output()
                .await
                .ok()?;
            let text = String::from_utf8_lossy(&out.stdout);
            crate::disk::parse_zypper_size(&text)
        })
    }

    fn supports_cleanup(&self) -> bool {
        true
    }

    fn run_cleanup<'a>(
        &'a self,
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            // Step 1: List orphaned packages (unprivileged).
            let list_out = match tokio::process::Command::new("zypper")
                .args(["--no-color", "packages", "--orphaned"])
                .env("LANG", "C")
                .env("LC_ALL", "C")
                .output()
                .await
            {
                Ok(o) => o,
                Err(e) => {
                    return UpdateResult::Error(BackendError::Spawn(e.to_string()));
                }
            };

            if !list_out.status.success() {
                return UpdateResult::Error(BackendError::Exit {
                    code: list_out.status.code().unwrap_or(-1),
                    message: String::from_utf8_lossy(&list_out.stderr).to_string(),
                });
            }

            let stdout = String::from_utf8_lossy(&list_out.stdout);
            let orphans: Vec<String> = parse_zypper_orphaned(&stdout)
                .into_iter()
                .filter(|n| is_safe_pkg_name(n))
                .collect();

            if orphans.is_empty() {
                return UpdateResult::Success { updated_count: 0 };
            }

            // Step 2: Remove orphans with privilege via a shell string.
            let pkg_list = orphans.join(" ");
            let cmd = format!("LANG=C LC_ALL=C zypper remove -y {}", pkg_list);
            let zypper_args = vec!["sh", "-c", cmd.as_str()];

            match runner.run("pkexec", &zypper_args).await {
                Ok(_) => UpdateResult::Success {
                    updated_count: orphans.len(),
                },
                Err(e) => UpdateResult::Error(e),
            }
        })
    }

    fn supports_item_selection(&self) -> bool {
        true
    }

    fn run_selected_update<'a>(
        &'a self,
        items: &'a [String],
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            // Validate: package names must be safe shell tokens.
            for pkg in items {
                if pkg.is_empty()
                    || pkg.len() > 255
                    || pkg
                        .chars()
                        .any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_'))
                {
                    return UpdateResult::Error(BackendError::from_string(format!(
                        "Invalid package name: {:?}",
                        pkg
                    )));
                }
            }
            let mut args = vec!["zypper", "--non-interactive", "update"];
            args.extend(items.iter().map(|s| s.as_str()));
            match runner.run("pkexec", &args).await {
                Ok(output) => UpdateResult::Success {
                    updated_count: count_zypper_upgraded(&output),
                },
                Err(e) => UpdateResult::Error(e),
            }
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

pub(crate) fn count_apt_autoremovals(output: &str) -> usize {
    // apt autoremove output: "N to remove" or "0 upgraded, 0 newly installed, N to remove"
    for line in output.lines() {
        if line.contains("to remove") {
            for word in line.split_whitespace() {
                if let Ok(n) = word.parse::<usize>() {
                    return n;
                }
            }
        }
    }
    0
}

pub(crate) fn count_dnf_autoremovals(output: &str) -> usize {
    // DNF4: "  Remove  N Packages"
    // DNF5: "  Removing: N packages"
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Remove ") || trimmed.starts_with("Removing:") {
            for word in trimmed.split_whitespace() {
                if let Ok(n) = word.parse::<usize>() {
                    return n;
                }
            }
        }
    }
    0
}

pub(crate) fn parse_zypper_orphaned(output: &str) -> Vec<String> {
    // `zypper packages --orphaned` uses the same pipe-delimited table format.
    // Lines starting with "i" or "i " (after trim) mark installed packages.
    output
        .lines()
        .filter(|l| l.trim_start().starts_with("i ") || l.trim_start().starts_with("i|"))
        .filter_map(|l| l.split('|').nth(2).map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect()
}

pub(crate) fn is_safe_pkg_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
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
