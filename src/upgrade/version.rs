use super::detect::DistroInfo;
use std::process::Command;
use std::time::Duration;

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

/// Construct a shared HTTP agent with a 10-second global timeout.
fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .new_agent()
}

/// Fetch the Ubuntu meta-release file via curl and return its content.
fn fetch_ubuntu_meta_release(policy: &str) -> Result<String, String> {
    let url = match policy {
        "normal" => "https://changelogs.ubuntu.com/meta-release",
        _ => "https://changelogs.ubuntu.com/meta-release-lts",
    };
    let agent = http_agent();
    let body = agent
        .get(url)
        .call()
        .map_err(|e| format!("HTTP request failed: {e}"))?
        .into_body()
        .read_to_string()
        .map_err(|e| format!("meta-release response is not valid UTF-8: {e}"))?;
    Ok(body)
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
    let url = format!(
        "https://dl.fedoraproject.org/pub/fedora/linux/releases/{}/Everything/x86_64/os/",
        next
    );
    let agent = http_agent();
    match agent.get(&url).call() {
        Ok(_) => format!("Yes — Fedora {} is available", next),
        Err(ureq::Error::StatusCode(_)) => format!("No — Fedora {} not yet released", next),
        Err(e) => {
            log::warn!("Could not check Fedora upgrade availability: {e}");
            format!("Could not check for Fedora upgrade: {e}")
        }
    }
}

/// Compute the next openSUSE Leap version from a "X.Y" `version_id`.
///
/// openSUSE Leap increments the minor component: 15.5 → 15.6.
/// The curl availability check acts as the authoritative gate for whether
/// the computed next version actually exists.
pub(crate) fn next_opensuse_leap_version(version_id: &str) -> Option<String> {
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
    let url = format!(
        "https://download.opensuse.org/distribution/leap/{}/repo/oss/",
        next_version
    );
    let agent = http_agent();
    match agent.get(&url).call() {
        Ok(_) => format!("Yes \u{2014} openSUSE Leap {} is available", next_version),
        Err(ureq::Error::StatusCode(_)) => format!(
            "No \u{2014} openSUSE Leap {} not yet released",
            next_version
        ),
        Err(e) => {
            log::warn!("Could not check openSUSE upgrade availability: {e}");
            format!("Could not check for openSUSE Leap upgrade: {e}")
        }
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

/// Validates that a hostname contains only characters safe for use as a NixOS
/// flake output attribute (`[a-zA-Z0-9\-_.]`).
///
/// This mirrors the identical guard in `src/backends/nix.rs`. Both upgrade
/// paths that embed a hostname in `/etc/nixos#<hostname>` must apply this
/// check before constructing the flake reference.
#[allow(dead_code)]
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

fn check_nixos_upgrade(current_version_id: &str) -> String {
    let Some(next_channel) = next_nixos_channel(current_version_id) else {
        return "Could not parse current NixOS version".to_string();
    };
    let version_label = next_channel.trim_start_matches("nixos-");
    let url = format!("https://channels.nixos.org/{}", next_channel);
    let agent = http_agent();
    match agent.get(&url).call() {
        Ok(_) => format!("Yes — NixOS {} is available", version_label),
        Err(ureq::Error::StatusCode(_)) => {
            format!("No — NixOS {} not yet available", version_label)
        }
        Err(e) => {
            log::warn!("Could not check NixOS upgrade availability: {e}");
            format!("Could not check for NixOS upgrade: {e}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{next_nixos_channel, next_opensuse_leap_version, validate_hostname};

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
