use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistroInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub version_id: String,
    pub upgrade_supported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub name: String,
    pub passed: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NixOsConfigType {
    Flake,
    LegacyChannel,
}

pub fn detect_nixos_config_type() -> NixOsConfigType {
    if std::path::Path::new("/etc/nixos/flake.nix").exists() {
        NixOsConfigType::Flake
    } else {
        NixOsConfigType::LegacyChannel
    }
}

pub fn detect_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .unwrap_or_else(|_| "nixos".to_owned())
        .trim()
        .to_string()
}

/// Validates that a hostname contains only characters safe for use as a NixOS
/// flake output attribute (`[a-zA-Z0-9\-_.]`).
///
/// This mirrors the identical guard in `src/backends/nix.rs`. Both upgrade
/// paths that embed a hostname in `/etc/nixos#<hostname>` must apply this
/// check before constructing the flake reference.
fn validate_hostname(hostname: &str) -> Result<&str, String> {
    if hostname.is_empty() {
        return Err("hostname is empty".to_string());
    }
    if hostname.len() > 253 {
        return Err(format!(
            "hostname is too long ({} chars, max 253)",
            hostname.len()
        ));
    }
    if !hostname
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(format!("Invalid hostname: {:?}", hostname));
    }
    Ok(hostname)
}

/// Parse /etc/os-release to detect the current distro.
pub fn detect_distro() -> DistroInfo {
    let os_release = fs::read_to_string("/etc/os-release").unwrap_or_default();
    let fields = parse_os_release(&os_release);

    let id = fields
        .get("ID")
        .cloned()
        .unwrap_or_else(|| "unknown".into());
    let name = fields
        .get("NAME")
        .cloned()
        .unwrap_or_else(|| "Unknown Linux".into());
    let version = fields
        .get("VERSION")
        .cloned()
        .unwrap_or_else(|| "Unknown".into());
    let version_id = fields
        .get("VERSION_ID")
        .cloned()
        .unwrap_or_else(|| "0".into());

    let upgrade_supported = matches!(
        id.as_str(),
        "ubuntu" | "fedora" | "opensuse-leap" | "debian" | "nixos"
    );

    DistroInfo {
        id,
        name,
        version,
        version_id,
        upgrade_supported,
    }
}

fn parse_os_release(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let value = value.trim_matches('"').to_string();
            map.insert(key.to_string(), value);
        }
    }
    map
}

/// Run prerequisite checks before an upgrade.
pub fn run_prerequisite_checks(
    distro: &DistroInfo,
    tx: &async_channel::Sender<String>,
) -> Vec<CheckResult> {
    let mut results = Vec::new();

    // Check 1: All packages up to date (or nixos-rebuild available for NixOS)
    let _ = tx.send_blocking("Checking if all packages are up to date...".into());
    let packages_ok = if distro.id == "nixos" {
        check_nixos_rebuild_available()
    } else {
        check_packages_up_to_date(distro)
    };
    results.push(packages_ok);

    // Check 2: Disk space
    let _ = tx.send_blocking("Checking available disk space...".into());
    let disk_ok = check_disk_space();
    results.push(disk_ok);

    // Check 3: Backup reminder (always passes, it's advisory)
    let _ = tx.send_blocking("Backup check...".into());
    results.push(CheckResult {
        name: "Backup recommended".into(),
        passed: true,
        message: "Please ensure you have a recent backup".into(),
    });

    results
}

fn check_packages_up_to_date(distro: &DistroInfo) -> CheckResult {
    let (cmd, args): (&str, &[&str]) = match distro.id.as_str() {
        "ubuntu" | "debian" => ("apt", &["list", "--upgradable"]),
        "fedora" => ("dnf", &["check-update"]),
        "opensuse-leap" => ("zypper", &["list-updates"]),
        "nixos" => {
            return CheckResult {
                name: "All packages up to date".into(),
                passed: true,
                message: "nixos-rebuild will apply all channel/flake updates".into(),
            };
        }
        _ => {
            return CheckResult {
                name: "All packages up to date".into(),
                passed: false,
                message: "Cannot check for this distro".into(),
            };
        }
    };

    match Command::new(cmd).args(args).output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let upgradable = stdout
                .lines()
                .filter(|l| !l.is_empty() && !l.starts_with("Listing"))
                .count();

            // `apt list --upgradable` always exits with code 0 regardless of
            // whether updates are pending; `output.status.success()` is therefore
            // always true for APT and must NOT be used as the pass condition.
            // The correct indicator for all supported tools (APT, DNF, Zypper)
            // is whether any package lines appear in their output.
            if upgradable == 0 {
                CheckResult {
                    name: "All packages up to date".into(),
                    passed: true,
                    message: "All packages are current".into(),
                }
            } else {
                CheckResult {
                    name: "All packages up to date".into(),
                    passed: false,
                    message: format!("{upgradable} packages need updating first"),
                }
            }
        }
        Err(e) => CheckResult {
            name: "All packages up to date".into(),
            passed: false,
            message: format!("Could not check: {e}"),
        },
    }
}

fn check_disk_space() -> CheckResult {
    // Check available space on /
    match Command::new("df")
        .args(["--output=avail", "-B1", "/"])
        .output()
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let avail_bytes: u64 = stdout
                .lines()
                .nth(1) // skip header
                .and_then(|l| l.trim().parse().ok())
                .unwrap_or(0);

            let avail_gb = avail_bytes / (1024 * 1024 * 1024);
            let required_gb = 10;

            if avail_gb >= required_gb {
                CheckResult {
                    name: "Sufficient disk space".into(),
                    passed: true,
                    message: format!("{avail_gb} GB available"),
                }
            } else {
                CheckResult {
                    name: "Sufficient disk space".into(),
                    passed: false,
                    message: format!("Only {avail_gb} GB available, {required_gb} GB recommended"),
                }
            }
        }
        Err(e) => CheckResult {
            name: "Sufficient disk space".into(),
            passed: false,
            message: format!("Could not check: {e}"),
        },
    }
}

/// Check if a distribution upgrade is available.
pub fn check_upgrade_available(distro: &DistroInfo) -> String {
    match distro.id.as_str() {
        "ubuntu" => check_ubuntu_upgrade(),
        "fedora" => check_fedora_upgrade(&distro.version_id),
        "debian" => check_debian_upgrade(),
        "opensuse-leap" => check_opensuse_upgrade(),
        "nixos" => check_nixos_upgrade(&distro.version_id),
        _ => "Not supported for this distribution".to_string(),
    }
}

fn check_ubuntu_upgrade() -> String {
    match Command::new("do-release-upgrade").args(["-c"]).output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("New release") || stdout.contains("new release") {
                let line = stdout
                    .lines()
                    .find(|l| l.contains("New release") || l.contains("new release"))
                    .unwrap_or("New release available");
                format!("Yes — {}", line.trim())
            } else {
                "No upgrade available".to_string()
            }
        }
        Err(_) => "Could not check (do-release-upgrade not found)".to_string(),
    }
}

fn check_fedora_upgrade(current_version_id: &str) -> String {
    let current: u32 = current_version_id.parse().unwrap_or(0);
    let next = current + 1;
    match Command::new("curl")
        .args([
            "-s",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            &format!(
                "https://dl.fedoraproject.org/pub/fedora/linux/releases/{}/Everything/x86_64/os/",
                next
            ),
        ])
        .output()
    {
        Ok(output) => {
            let code = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if code == "200" || code == "301" || code == "302" {
                format!("Yes — Fedora {} is available", next)
            } else {
                format!("No — Fedora {} not yet released", next)
            }
        }
        Err(_) => "Could not check (curl not found)".to_string(),
    }
}

fn check_debian_upgrade() -> String {
    "Check manually at https://www.debian.org/releases/".to_string()
}

fn check_opensuse_upgrade() -> String {
    "Check manually at https://get.opensuse.org/leap/".to_string()
}

fn check_nixos_upgrade(current_version_id: &str) -> String {
    let parts: Vec<&str> = current_version_id.split('.').collect();
    if parts.len() == 2 {
        if let (Ok(year), Ok(month)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
            let (next_year, next_month) = if month >= 11 {
                (year + 1, 5)
            } else {
                (year, 11)
            };
            let next_channel = format!("nixos-{}.{:02}", next_year, next_month);
            match Command::new("curl")
                .args([
                    "-s",
                    "-o",
                    "/dev/null",
                    "-w",
                    "%{http_code}",
                    &format!("https://channels.nixos.org/{}", next_channel),
                ])
                .output()
            {
                Ok(output) => {
                    let code = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if code == "200" || code == "301" || code == "302" {
                        format!("Yes — NixOS {}.{:02} is available", next_year, next_month)
                    } else {
                        format!(
                            "No — NixOS {}.{:02} not yet available",
                            next_year, next_month
                        )
                    }
                }
                Err(_) => "Could not check (curl not found)".to_string(),
            }
        } else {
            "Could not parse current NixOS version".to_string()
        }
    } else {
        "Could not parse current NixOS version".to_string()
    }
}

/// Execute the actual distro upgrade.
/// Returns `true` if all upgrade steps completed successfully, `false` otherwise.
pub fn execute_upgrade(distro: &DistroInfo, tx: &async_channel::Sender<String>) -> bool {
    let _ = tx.send_blocking(format!(
        "Starting upgrade for {} {}...",
        distro.name, distro.version
    ));

    match distro.id.as_str() {
        "ubuntu" | "debian" => upgrade_ubuntu(tx),
        "fedora" => upgrade_fedora(tx),
        "opensuse-leap" => upgrade_opensuse(tx),
        "nixos" => upgrade_nixos(tx),
        _ => {
            let _ = tx.send_blocking(format!(
                "Upgrade is not yet supported for '{}'. Supported: Ubuntu, Fedora, openSUSE Leap, NixOS.",
                distro.name
            ));
            false
        }
    }
}

fn upgrade_ubuntu(tx: &async_channel::Sender<String>) -> bool {
    let _ = tx.send_blocking("Running: do-release-upgrade -f DistUpgradeViewNonInteractive".into());

    crate::runner::run_command_sync(
        "pkexec",
        &["do-release-upgrade", "-f", "DistUpgradeViewNonInteractive"],
        tx,
    )
}

fn upgrade_fedora(tx: &async_channel::Sender<String>) -> bool {
    // Step 1: Install upgrade plugin
    let _ = tx.send_blocking("Installing system-upgrade plugin...".into());
    if !crate::runner::run_command_sync(
        "pkexec",
        &["dnf", "install", "-y", "dnf-plugin-system-upgrade"],
        tx,
    ) {
        return false;
    }

    // Step 2: Download upgrade packages (next version)
    let _ = tx.send_blocking("Downloading upgrade packages...".into());

    // Detect next version
    let next_version = match detect_next_fedora_version() {
        Some(v) => v,
        None => {
            let _ = tx.send_blocking(
                "Error: Could not detect current Fedora version. Aborting upgrade.".into(),
            );
            return false;
        }
    };
    let ver_str = next_version.to_string();
    if !crate::runner::run_command_sync(
        "pkexec",
        &[
            "dnf",
            "system-upgrade",
            "download",
            "--releasever",
            &ver_str,
            "-y",
        ],
        tx,
    ) {
        return false;
    }

    // Step 3: Trigger reboot into upgrade
    let _ =
        tx.send_blocking("Download complete. The system will reboot to apply the upgrade.".into());
    crate::runner::run_command_sync("pkexec", &["dnf", "system-upgrade", "reboot"], tx)
}

fn upgrade_opensuse(tx: &async_channel::Sender<String>) -> bool {
    let _ = tx.send_blocking("Running zypper distribution upgrade...".into());
    crate::runner::run_command_sync("pkexec", &["zypper", "dup", "-y"], tx)
}

fn upgrade_nixos(tx: &async_channel::Sender<String>) -> bool {
    // pkexec resets PATH, excluding NixOS tooling; export the required paths explicitly.
    const NIX_PATH_EXPORT: &str =
        "export PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin:$PATH";
    let config_type = detect_nixos_config_type();
    match config_type {
        NixOsConfigType::LegacyChannel => {
            let _ = tx.send_blocking("Detected: legacy channel-based NixOS config".into());
            let _ = tx.send_blocking("Updating NixOS channel...".into());
            let cmd = format!("{NIX_PATH_EXPORT} && nix-channel --update");
            if !crate::runner::run_command_sync("pkexec", &["sh", "-c", &cmd], tx) {
                return false;
            }
            let _ = tx.send_blocking("Rebuilding NixOS (switch --upgrade)...".into());
            crate::runner::run_command_sync("pkexec", &["nixos-rebuild", "switch", "--upgrade"], tx)
        }
        NixOsConfigType::Flake => {
            let _ = tx.send_blocking("Detected: flake-based NixOS config".into());
            let _ = tx.send_blocking("Updating flake inputs in /etc/nixos...".into());
            let cmd = format!("{NIX_PATH_EXPORT} && nix flake update --flake /etc/nixos");
            if !crate::runner::run_command_sync("pkexec", &["sh", "-c", &cmd], tx) {
                return false;
            }
            // Validate before embedding in the Nix flake URL. An unvalidated hostname
            // containing '#', '?', spaces, or control characters can confuse
            // nixos-rebuild's flake-reference parser and may cause an incorrect rebuild
            // or a cryptic failure. This mirrors the identical guard in nix.rs.
            let raw_hostname = detect_hostname();
            let hostname = match validate_hostname(&raw_hostname) {
                Ok(h) => h,
                Err(e) => {
                    let _ = tx.send_blocking(format!("Upgrade aborted: {e}"));
                    return false;
                }
            };
            let flake_target = format!("/etc/nixos#{}", hostname);
            let _ = tx.send_blocking(format!("Rebuilding NixOS configuration: {flake_target}"));
            crate::runner::run_command_sync(
                "pkexec",
                &["nixos-rebuild", "switch", "--flake", &flake_target],
                tx,
            )
        }
    }
}

fn check_nixos_rebuild_available() -> CheckResult {
    if which::which("nixos-rebuild").is_ok() {
        CheckResult {
            name: "nixos-rebuild available".into(),
            passed: true,
            message: "nixos-rebuild is installed and accessible".into(),
        }
    } else {
        CheckResult {
            name: "nixos-rebuild available".into(),
            passed: false,
            message: "nixos-rebuild not found; ensure NixOS system tools are installed".into(),
        }
    }
}

fn detect_next_fedora_version() -> Option<u32> {
    // Primary: rpm macro (most accurate on Fedora)
    if let Ok(output) = Command::new("rpm").args(["-E", "%fedora"]).output() {
        let s = String::from_utf8_lossy(&output.stdout);
        let trimmed = s.trim();
        // Only accept it if it looks like a plain number (not the unexpanded macro "%fedora")
        if !trimmed.starts_with('%') {
            if let Ok(n) = trimmed.parse::<u32>() {
                return Some(n + 1);
            }
        }
    }
    // Fallback: parse VERSION_ID from /etc/os-release
    if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
        for line in content.lines() {
            if let Some(val) = line.strip_prefix("VERSION_ID=") {
                let val = val.trim_matches('"');
                if let Ok(n) = val.parse::<u32>() {
                    return Some(n + 1);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::validate_hostname;

    #[test]
    fn validate_hostname_rejects_dangerous_input() {
        // Empty
        assert!(validate_hostname("").is_err());
        // Too long (254 chars)
        assert!(validate_hostname(&"a".repeat(254)).is_err());
        // '#' splits the Nix flake attribute path
        assert!(validate_hostname("host#evil").is_err());
        // '?' is parsed as a Nix flake URL query parameter
        assert!(validate_hostname("host?url=override").is_err());
        // Space is not valid in a flake attr path
        assert!(validate_hostname("my host").is_err());
        // NUL byte
        assert!(validate_hostname("host\x00name").is_err());
        // Newline
        assert!(validate_hostname("host\nmalicious").is_err());
        // Shell metacharacters (defense in depth)
        assert!(validate_hostname("host;id").is_err());
    }

    #[test]
    fn validate_hostname_accepts_valid_input() {
        assert!(validate_hostname("nixos").is_ok());
        assert!(validate_hostname("my-server").is_ok());
        assert!(validate_hostname("server1.local").is_ok());
        assert!(validate_hostname("MY_SERVER_42").is_ok());
        // Underscore is common in NixOS hostnames
        assert!(validate_hostname("my_host").is_ok());
        assert!(validate_hostname("a").is_ok());
        // Exactly 253 chars — boundary must pass
        assert!(validate_hostname(&"a".repeat(253)).is_ok());
    }
}
