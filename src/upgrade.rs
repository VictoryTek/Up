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

/// Carries all detection results the upgrade page needs to initialise.
/// Sent once from UpWindow::build() over a bounded channel after detection.
#[derive(Debug, Clone)]
pub struct UpgradePageInit {
    pub distro: DistroInfo,
    pub nixos_extra: Option<(NixOsConfigType, String)>,
}

/// Structured result of an Ubuntu upgrade availability check.
#[derive(Debug, Clone)]
pub enum UbuntuUpgradeInfo {
    /// A newer Ubuntu release is available and the upgrade path is officially open.
    Available { name: String, version: String },
    /// A newer Ubuntu release has been released but Canonical has not yet opened
    /// the upgrade path (Supported: 0 in meta-release). Typically takes 4-8 weeks
    /// after release before Canonical opens the LTS upgrade path.
    ReleasedNotPromoted { name: String, version: String },
    /// No newer Ubuntu release exists in the meta-release file.
    NotAvailable,
    /// The check could not be completed (network error, missing curl, parse error).
    CheckFailed(String),
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

    let id_like = fields.get("ID_LIKE").cloned().unwrap_or_default();

    let upgrade_supported = match id.as_str() {
        "ubuntu" | "linuxmint" | "pop" | "elementary" | "zorin" => true,
        "fedora" => true,
        "opensuse-leap" => true,
        "debian" => true,
        "nixos" => true,
        "rhel" | "centos" => true,
        _ if id_like.split_whitespace().any(|s| s == "ubuntu") => true,
        _ if id_like.split_whitespace().any(|s| s == "debian") => true,
        _ => false,
    };

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
        "ubuntu" => ("apt", &["list", "--upgradable"]),
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

fn parse_df_avail_bytes(stdout: &str) -> Result<u64, String> {
    let line = stdout
        .lines()
        .nth(1) // skip header
        .ok_or_else(|| "df output contains no data line".to_string())?;
    let trimmed = line.trim();
    trimmed
        .parse::<u64>()
        .map_err(|e| format!("could not parse {:?} as bytes: {e}", trimmed))
}

fn check_disk_space() -> CheckResult {
    // Check available space on /
    match Command::new("df")
        .args(["--output=avail", "-B1", "/"])
        .output()
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            match parse_df_avail_bytes(&stdout) {
                Err(reason) => CheckResult {
                    name: "Sufficient disk space".into(),
                    passed: false,
                    message: format!("Could not parse disk space output: {reason}"),
                },
                Ok(avail_bytes) => {
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
        "ubuntu" => check_ubuntu_upgrade(&distro.version_id),
        "fedora" => check_fedora_upgrade(&distro.version_id),
        "opensuse-leap" => check_opensuse_upgrade(&distro.version_id),
        "nixos" => check_nixos_upgrade(&distro.version_id),
        _ => "Not supported for this distribution".to_string(),
    }
}

/// Read /etc/update-manager/release-upgrades and return the Prompt= value.
/// Returns "lts" as default if the file is missing or unparseable.
fn read_upgrade_prompt_policy() -> String {
    let content =
        std::fs::read_to_string("/etc/update-manager/release-upgrades").unwrap_or_default();
    for line in content.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("Prompt=") {
            let v = val.trim().to_lowercase();
            if v == "lts" || v == "normal" || v == "never" {
                return v;
            }
        }
    }
    "lts".to_string()
}

/// Parse an Ubuntu version string "X.YY" or "X.YY LTS" into (major, minor).
fn parse_ubuntu_version(version: &str) -> Option<(u32, u32)> {
    let numeric = version.split_whitespace().next()?;
    let mut parts = numeric.splitn(2, '.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Fetch the Ubuntu meta-release file via curl and return its content.
fn fetch_ubuntu_meta_release(policy: &str) -> Result<String, String> {
    let url = match policy {
        "normal" => "https://changelogs.ubuntu.com/meta-release",
        _ => "https://changelogs.ubuntu.com/meta-release-lts",
    };
    let output = Command::new("curl")
        .args(["-sf", "--max-time", "10", url])
        .output()
        .map_err(|e| format!("curl not found: {e}"))?;
    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        return Err(format!("curl exited with code {code}"));
    }
    String::from_utf8(output.stdout)
        .map_err(|e| format!("meta-release response is not valid UTF-8: {e}"))
}

/// Parse the Ubuntu meta-release content and find the first release newer than
/// `current_version_id` (e.g., "24.04").
fn parse_meta_release_for_upgrade(content: &str, current_version_id: &str) -> UbuntuUpgradeInfo {
    let current = match parse_ubuntu_version(current_version_id) {
        Some(v) => v,
        None => {
            return UbuntuUpgradeInfo::CheckFailed(format!(
                "Cannot parse current version: {:?}",
                current_version_id
            ))
        }
    };

    for block in content.split("\n\n") {
        let mut name = String::new();
        let mut version_str = String::new();
        let mut supported: i32 = -1;

        for line in block.lines() {
            let line = line.trim();
            if let Some(v) = line.strip_prefix("Name: ") {
                name = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("Version: ") {
                version_str = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("Supported: ") {
                supported = v.trim().parse().unwrap_or(-1);
            }
        }

        if version_str.is_empty() {
            continue;
        }

        let candidate = match parse_ubuntu_version(&version_str) {
            Some(v) => v,
            None => continue,
        };

        if candidate > current {
            return if supported == 1 {
                UbuntuUpgradeInfo::Available {
                    name,
                    version: version_str,
                }
            } else {
                UbuntuUpgradeInfo::ReleasedNotPromoted {
                    name,
                    version: version_str,
                }
            };
        }
    }

    UbuntuUpgradeInfo::NotAvailable
}

/// Fallback upgrade check using do-release-upgrade -c when curl is unavailable.
/// Returns Some(message) if the tool is available, None otherwise.
fn check_ubuntu_upgrade_via_tool() -> Option<String> {
    let output = Command::new("do-release-upgrade")
        .args(["-c", "-f", "DistUpgradeViewNonInteractive"])
        .output()
        .ok()?;

    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));

    if combined.contains("New release") || combined.contains("new release") {
        let line = combined
            .lines()
            .find(|l| l.contains("New release") || l.contains("new release"))
            .unwrap_or("New release available");
        Some(format!("Yes \u{2014} {}", line.trim()))
    } else if combined.contains("No new release") {
        Some("No \u{2014} No newer Ubuntu release available".to_string())
    } else {
        Some("No \u{2014} No upgrade available".to_string())
    }
}

fn check_ubuntu_upgrade(version_id: &str) -> String {
    let policy = read_upgrade_prompt_policy();

    if policy == "never" {
        return "Upgrades are disabled in /etc/update-manager/release-upgrades".to_string();
    }

    match fetch_ubuntu_meta_release(&policy) {
        Err(e) => check_ubuntu_upgrade_via_tool()
            .unwrap_or_else(|| format!("Could not check for upgrades: {e}")),
        Ok(content) => match parse_meta_release_for_upgrade(&content, version_id) {
            UbuntuUpgradeInfo::Available { name, version } => {
                format!("Yes \u{2014} {} {} is available", name, version)
            }
            UbuntuUpgradeInfo::ReleasedNotPromoted { name, version } => {
                format!(
                    "No \u{2014} {} {} is released but the upgrade is not yet available. \
                     Canonical typically opens the LTS upgrade path 4\u{2013}8 weeks \
                     after release.",
                    name, version
                )
            }
            UbuntuUpgradeInfo::NotAvailable => {
                "No \u{2014} No newer Ubuntu release available".to_string()
            }
            UbuntuUpgradeInfo::CheckFailed(reason) => {
                format!("Could not check for upgrades: {}", reason)
            }
        },
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

/// Compute the next openSUSE Leap version from a "X.Y" `version_id`.
///
/// openSUSE Leap increments the minor component: 15.5 → 15.6.
/// The curl availability check acts as the authoritative gate for whether
/// the computed next version actually exists.
fn next_opensuse_leap_version(version_id: &str) -> Option<String> {
    let parts: Vec<&str> = version_id.split('.').collect();
    if parts.len() != 2 {
        return None;
    }
    let major: u32 = parts[0].parse().ok()?;
    let minor: u32 = parts[1].parse().ok()?;
    Some(format!("{}.{}", major, minor + 1))
}

fn check_opensuse_upgrade(version_id: &str) -> String {
    let Some(next_version) = next_opensuse_leap_version(version_id) else {
        return "Could not parse current openSUSE Leap version".to_string();
    };
    match Command::new("curl")
        .args([
            "-s",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            &format!(
                "https://download.opensuse.org/distribution/leap/{}/repo/oss/",
                next_version
            ),
        ])
        .output()
    {
        Ok(output) => {
            let code = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if code == "200" || code == "301" || code == "302" {
                format!("Yes \u{2014} openSUSE Leap {} is available", next_version)
            } else {
                format!(
                    "No \u{2014} openSUSE Leap {} not yet released",
                    next_version
                )
            }
        }
        Err(_) => "Could not check (curl not found)".to_string(),
    }
}

/// Compute the next NixOS stable channel name from a YY.MM `version_id`.
///
/// Returns `Some("nixos-YY.MM")` for the next release, or `None` if the
/// version_id cannot be parsed.
///
/// NixOS releases every six months: May (05) and November (11).
/// - If current month is ≥ 11, next is (year+1, 05)
/// - Otherwise, next is (year, 11)
pub fn next_nixos_channel(version_id: &str) -> Option<String> {
    let parts: Vec<&str> = version_id.split('.').collect();
    if parts.len() != 2 {
        return None;
    }
    let year: u32 = parts[0].parse().ok()?;
    let month: u32 = parts[1].parse().ok()?;
    let (ny, nm) = if month >= 11 {
        (year + 1, 5)
    } else {
        (year, 11)
    };
    Some(format!("nixos-{}.{:02}", ny, nm))
}

fn check_nixos_upgrade(current_version_id: &str) -> String {
    let Some(next_channel) = next_nixos_channel(current_version_id) else {
        return "Could not parse current NixOS version".to_string();
    };
    let version_label = next_channel.trim_start_matches("nixos-");
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
                format!("Yes — NixOS {} is available", version_label)
            } else {
                format!("No — NixOS {} not yet available", version_label)
            }
        }
        Err(_) => "Could not check (curl not found)".to_string(),
    }
}

/// Execute the actual distro upgrade.
/// Returns `Ok(())` if all upgrade steps completed successfully, or `Err(reason)` otherwise.
pub fn execute_upgrade(
    distro: &DistroInfo,
    tx: &async_channel::Sender<String>,
) -> Result<(), String> {
    let _ = tx.send_blocking(format!(
        "Starting upgrade for {} {}...",
        distro.name, distro.version
    ));

    match distro.id.as_str() {
        "ubuntu" => upgrade_ubuntu(tx),
        "fedora" => upgrade_fedora(tx),
        "opensuse-leap" => upgrade_opensuse(tx),
        "nixos" => upgrade_nixos(distro, tx),
        _ => {
            let msg = format!(
                "Upgrade is not yet supported for '{}'. Supported: Ubuntu, Fedora, openSUSE Leap, NixOS.",
                distro.name
            );
            let _ = tx.send_blocking(msg.clone());
            Err(msg)
        }
    }
}

fn upgrade_ubuntu(tx: &async_channel::Sender<String>) -> Result<(), String> {
    let _ = tx.send_blocking("Preparing Ubuntu distribution upgrade...".into());
    let _ = tx.send_blocking(
        "This operation downloads and installs many packages. It may take 30\u{2013}60 \
         minutes. Do not power off the system."
            .into(),
    );

    let log_path = "/var/log/dist-upgrade/main.log";
    let tx_tail = tx.clone();
    let tail_handle = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        use std::io::{BufRead, BufReader, Seek, SeekFrom};
        let Ok(mut file) = std::fs::File::open(log_path) else {
            return;
        };
        let _ = file.seek(SeekFrom::End(0));
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        loop {
            match reader.read_line(&mut line) {
                Ok(0) => {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                Ok(_) => {
                    let trimmed = line.trim_end_matches('\n').to_string();
                    if !trimmed.is_empty() {
                        let _ = tx_tail.send_blocking(format!("[log] {}", trimmed));
                    }
                    line.clear();
                }
                Err(_) => break,
            }
        }
    });

    let result = if !crate::runner::run_command_sync(
        "pkexec",
        &[
            "do-release-upgrade",
            "-f",
            "DistUpgradeViewNonInteractive",
            "-e",
            "DEBIAN_FRONTEND=noninteractive",
        ],
        tx,
    ) {
        Err("Ubuntu distribution upgrade failed (see log for details)".to_string())
    } else {
        Ok(())
    };

    drop(tail_handle);
    result
}

fn upgrade_fedora(tx: &async_channel::Sender<String>) -> Result<(), String> {
    // Step 1: Install upgrade plugin
    let _ = tx.send_blocking("Installing system-upgrade plugin...".into());
    if !crate::runner::run_command_sync(
        "pkexec",
        &["dnf", "install", "-y", "dnf-plugin-system-upgrade"],
        tx,
    ) {
        return Err(
            "Failed to install dnf-plugin-system-upgrade (see log for details)".to_string(),
        );
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
            return Err(
                "Could not detect current Fedora version to determine upgrade target".to_string(),
            );
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
        return Err(format!(
            "Failed to download Fedora {} upgrade packages (see log for details)",
            next_version
        ));
    }

    // Step 3: Trigger reboot into upgrade
    let _ =
        tx.send_blocking("Download complete. The system will reboot to apply the upgrade.".into());
    if !crate::runner::run_command_sync("pkexec", &["dnf", "system-upgrade", "reboot"], tx) {
        return Err("Failed to trigger Fedora upgrade reboot (see log for details)".to_string());
    }
    Ok(())
}

fn upgrade_opensuse(tx: &async_channel::Sender<String>) -> Result<(), String> {
    let _ = tx.send_blocking("Running zypper distribution upgrade...".into());
    if !crate::runner::run_command_sync("pkexec", &["zypper", "dup", "-y"], tx) {
        return Err(
            "openSUSE distribution upgrade command failed (see log for details)".to_string(),
        );
    }
    Ok(())
}

fn upgrade_nixos(distro: &DistroInfo, tx: &async_channel::Sender<String>) -> Result<(), String> {
    // pkexec resets PATH, excluding NixOS tooling; export the required paths explicitly.
    const NIX_PATH_EXPORT: &str =
        "export PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin:$PATH";
    let config_type = detect_nixos_config_type();
    match config_type {
        NixOsConfigType::LegacyChannel => {
            let _ = tx.send_blocking("Detected: legacy channel-based NixOS config".into());

            // Determine the target channel
            let next_channel = match next_nixos_channel(&distro.version_id) {
                Some(ch) => ch,
                None => {
                    let msg = format!(
                        "Cannot determine next NixOS channel from version '{}'",
                        distro.version_id
                    );
                    let _ = tx.send_blocking(msg.clone());
                    return Err(msg);
                }
            };
            let channel_url = format!("https://nixos.org/channels/{}", next_channel);

            // Step 1: Register the new channel
            let _ = tx.send_blocking(format!("Switching channel to {}...", next_channel));
            let add_cmd = format!(
                "{NIX_PATH_EXPORT} && nix-channel --add {} nixos",
                channel_url
            );
            if !crate::runner::run_command_sync("pkexec", &["sh", "-c", &add_cmd], tx) {
                return Err(format!(
                    "Failed to register NixOS channel {} (see log for details)",
                    next_channel
                ));
            }

            // Step 2: Rebuild with --upgrade to apply the new channel
            let _ = tx.send_blocking(format!(
                "Rebuilding NixOS with {} (nixos-rebuild switch --upgrade)...",
                next_channel
            ));
            if !crate::runner::run_command_sync(
                "pkexec",
                &["nixos-rebuild", "switch", "--upgrade"],
                tx,
            ) {
                return Err(
                    "Failed to rebuild NixOS with --upgrade (see log for details)".to_string(),
                );
            }
            Ok(())
        }
        NixOsConfigType::Flake => {
            let _ = tx.send_blocking("Detected: flake-based NixOS config".into());
            let _ = tx.send_blocking("Updating flake inputs in /etc/nixos...".into());
            let cmd = format!("{NIX_PATH_EXPORT} && nix flake update --flake /etc/nixos");
            if !crate::runner::run_command_sync("pkexec", &["sh", "-c", &cmd], tx) {
                return Err(
                    "Failed to update flake inputs in /etc/nixos (see log for details)".to_string(),
                );
            }
            // Validate before embedding in the Nix flake URL. An unvalidated hostname
            // containing '#', '?', spaces, or control characters can confuse
            // nixos-rebuild's flake-reference parser and may cause an incorrect rebuild
            // or a cryptic failure. This mirrors the identical guard in nix.rs.
            let raw_hostname = detect_hostname();
            let hostname = match validate_hostname(&raw_hostname) {
                Ok(h) => h,
                Err(e) => {
                    let msg = format!("Upgrade aborted: {e}");
                    let _ = tx.send_blocking(msg.clone());
                    return Err(msg);
                }
            };
            let flake_target = format!("/etc/nixos#{}", hostname);
            let _ = tx.send_blocking(format!("Rebuilding NixOS configuration: {flake_target}"));
            if !crate::runner::run_command_sync(
                "pkexec",
                &["nixos-rebuild", "switch", "--flake", &flake_target],
                tx,
            ) {
                return Err(format!(
                    "Failed to rebuild NixOS flake configuration '{}' (see log for details)",
                    flake_target
                ));
            }
            Ok(())
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
    use super::{
        execute_upgrade, next_nixos_channel, next_opensuse_leap_version, parse_df_avail_bytes,
        validate_hostname, DistroInfo,
    };

    #[test]
    fn next_nixos_channel_from_may_gives_november() {
        assert_eq!(next_nixos_channel("24.05"), Some("nixos-24.11".to_string()));
    }

    #[test]
    fn next_nixos_channel_from_november_gives_next_may() {
        assert_eq!(next_nixos_channel("24.11"), Some("nixos-25.05".to_string()));
    }

    #[test]
    fn next_nixos_channel_invalid_returns_none() {
        assert_eq!(next_nixos_channel("unstable"), None);
        assert_eq!(next_nixos_channel(""), None);
        assert_eq!(next_nixos_channel("24"), None);
    }

    #[test]
    fn execute_upgrade_unsupported_distro_returns_err() {
        let distro = DistroInfo {
            id: "arch".to_string(),
            name: "Arch Linux".to_string(),
            version: "2026.01.01".to_string(),
            version_id: "2026".to_string(),
            upgrade_supported: false,
        };
        let (tx, _rx) = async_channel::unbounded::<String>();
        let result = execute_upgrade(&distro, &tx);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("not yet supported"),
            "unexpected message: {msg}"
        );
    }

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

    #[test]
    fn parse_df_avail_bytes_normal() {
        // Typical df --output=avail -B1 output: header + data line
        let output = "     Avail\n10737418240\n";
        assert_eq!(parse_df_avail_bytes(output), Ok(10_737_418_240u64));
    }

    #[test]
    fn parse_df_avail_bytes_genuine_zero() {
        // Zero is a valid value — completely full disk
        let output = "     Avail\n0\n";
        assert_eq!(parse_df_avail_bytes(output), Ok(0u64));
    }

    #[test]
    fn parse_df_avail_bytes_empty_stdout() {
        // df spawned successfully but produced no output at all
        assert!(parse_df_avail_bytes("").is_err());
    }

    #[test]
    fn parse_df_avail_bytes_header_only() {
        // Output contains the header line but no data line
        let output = "     Avail\n";
        assert!(parse_df_avail_bytes(output).is_err());
    }

    #[test]
    fn parse_df_avail_bytes_non_numeric() {
        // Non-numeric content in the data line (e.g. BusyBox error on stdout)
        let output = "     Avail\nN/A\n";
        assert!(parse_df_avail_bytes(output).is_err());
    }

    #[test]
    fn parse_df_avail_bytes_locale_comma() {
        // Locale-formatted number — parse::<u64>() does not accept commas
        let output = "     Avail\n10,737,418,240\n";
        assert!(parse_df_avail_bytes(output).is_err());
    }

    #[test]
    fn next_opensuse_leap_version_increments_minor() {
        assert_eq!(next_opensuse_leap_version("15.5"), Some("15.6".to_string()));
        assert_eq!(next_opensuse_leap_version("15.6"), Some("15.7".to_string()));
    }

    #[test]
    fn next_opensuse_leap_version_invalid_returns_none() {
        assert_eq!(next_opensuse_leap_version("invalid"), None);
        assert_eq!(next_opensuse_leap_version(""), None);
        assert_eq!(next_opensuse_leap_version("15"), None);
    }
}
