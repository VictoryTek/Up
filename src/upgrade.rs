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

/// Parse /etc/os-release to detect the current distro.
pub fn detect_distro() -> DistroInfo {
    let os_release = fs::read_to_string("/etc/os-release").unwrap_or_default();
    let fields = parse_os_release(&os_release);

    let id = fields.get("ID").cloned().unwrap_or_else(|| "unknown".into());
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

    let upgrade_supported = matches!(id.as_str(), "ubuntu" | "fedora" | "opensuse-leap" | "debian" | "nixos");

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

            if upgradable == 0 || output.status.success() {
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
                    message: format!(
                        "Only {avail_gb} GB available, {required_gb} GB recommended"
                    ),
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

/// Execute the actual distro upgrade.
pub fn execute_upgrade(distro: &DistroInfo, tx: &async_channel::Sender<String>) {
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
        }
    }
}

fn upgrade_ubuntu(tx: &async_channel::Sender<String>) {
    let _ = tx.send_blocking("Running: do-release-upgrade -f DistUpgradeViewNonInteractive".into());

    run_streaming_command(
        "pkexec",
        &["do-release-upgrade", "-f", "DistUpgradeViewNonInteractive"],
        tx,
    );
}

fn upgrade_fedora(tx: &async_channel::Sender<String>) {
    // Step 1: Install upgrade plugin
    let _ = tx.send_blocking("Installing system-upgrade plugin...".into());
    run_streaming_command(
        "pkexec",
        &["dnf", "install", "-y", "dnf-plugin-system-upgrade"],
        tx,
    );

    // Step 2: Download upgrade packages (next version)
    let _ = tx.send_blocking("Downloading upgrade packages...".into());

    // Detect next version
    let next_version = detect_next_fedora_version();
    let ver_str = next_version.to_string();
    run_streaming_command(
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
    );

    // Step 3: Trigger reboot into upgrade
    let _ = tx.send_blocking(
        "Download complete. The system will reboot to apply the upgrade.".into(),
    );
    run_streaming_command("pkexec", &["dnf", "system-upgrade", "reboot"], tx);
}

fn upgrade_opensuse(tx: &async_channel::Sender<String>) {
    let _ = tx.send_blocking("Running zypper distribution upgrade...".into());
    run_streaming_command("pkexec", &["zypper", "dup", "-y"], tx);
}

fn upgrade_nixos(tx: &async_channel::Sender<String>) {
    let config_type = detect_nixos_config_type();
    match config_type {
        NixOsConfigType::LegacyChannel => {
            let _ = tx.send_blocking("Detected: legacy channel-based NixOS config".into());
            let _ = tx.send_blocking("Updating NixOS channel...".into());
            run_streaming_command("sudo", &["nix-channel", "--update"], tx);
            let _ = tx.send_blocking("Rebuilding NixOS (switch --upgrade)...".into());
            run_streaming_command("pkexec", &["nixos-rebuild", "switch", "--upgrade"], tx);
        }
        NixOsConfigType::Flake => {
            let _ = tx.send_blocking("Detected: flake-based NixOS config".into());
            let _ = tx.send_blocking("Updating flake inputs in /etc/nixos...".into());
            run_streaming_command("sudo", &["nix", "flake", "update", "--flake", "/etc/nixos"], tx);
            let hostname = detect_hostname();
            let flake_target = format!("/etc/nixos#{}", hostname);
            let _ = tx.send_blocking(format!("Rebuilding NixOS configuration: {flake_target}"));
            run_streaming_command(
                "pkexec",
                &["nixos-rebuild", "switch", "--flake", &flake_target],
                tx,
            );
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

fn detect_next_fedora_version() -> u32 {
    let output = Command::new("rpm")
        .args(["-E", "%fedora"])
        .output()
        .ok();

    if let Some(out) = output {
        let current: u32 = String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .unwrap_or(0);
        current + 1
    } else {
        // Fallback to a reasonable version
        41
    }
}

fn run_streaming_command(program: &str, args: &[&str], tx: &async_channel::Sender<String>) {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let result = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    match result {
        Ok(mut child) => {
            if let Some(stdout) = child.stdout.take() {
                let reader = BufReader::new(stdout);
                for line in reader.lines().map_while(Result::ok) {
                    let _ = tx.send_blocking(line);
                }
            }

            if let Some(stderr) = child.stderr.take() {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    let _ = tx.send_blocking(format!("stderr: {line}"));
                }
            }

            match child.wait() {
                Ok(status) => {
                    if status.success() {
                        let _ = tx.send_blocking("Command completed successfully.".into());
                    } else {
                        let code = status.code().unwrap_or(-1);
                        let _ = tx.send_blocking(format!("Command exited with code {code}"));
                    }
                }
                Err(e) => {
                    let _ = tx.send_blocking(format!("Failed to wait for process: {e}"));
                }
            }
        }
        Err(e) => {
            let _ = tx.send_blocking(format!("Failed to start {program}: {e}"));
        }
    }
}
