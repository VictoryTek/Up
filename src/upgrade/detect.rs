use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistroInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub version_id: String,
    pub upgrade_supported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NixOsConfigType {
    Flake,
    LegacyChannel,
}

/// The distro-upgrade implementation to use for a given `/etc/os-release` `ID`.
///
/// Single source of truth for "which distros can Up actually upgrade" — the
/// `upgrade_supported` flag, `execute::execute_upgrade`, and
/// `version::check_upgrade_available` all derive their dispatch from
/// [`UpgradeStrategy::for_distro`], so they can no longer disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeStrategy {
    Ubuntu,
    Fedora,
    OpenSuseLeap,
    NixOs,
}

impl UpgradeStrategy {
    /// Returns the upgrade strategy for a distro `ID`, or `None` when Up has no
    /// implemented upgrade path for it.
    pub fn for_distro(id: &str) -> Option<Self> {
        match id {
            "ubuntu" => Some(Self::Ubuntu),
            "fedora" => Some(Self::Fedora),
            "opensuse-leap" => Some(Self::OpenSuseLeap),
            "nixos" => Some(Self::NixOs),
            _ => None,
        }
    }
}

/// Carries all detection results the upgrade page needs to initialise.
/// Sent once from UpWindow::build() over a bounded channel after detection.
#[derive(Debug, Clone)]
pub struct UpgradePageInit {
    pub distro: DistroInfo,
    pub nixos_extra: Option<(NixOsConfigType, String)>,
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

    // Single source of truth — see `UpgradeStrategy::for_distro`. A distro is
    // "supported" only when an actual upgrade path is implemented for it;
    // otherwise the UI would let the user reach a "not implemented" dead end.
    let upgrade_supported = UpgradeStrategy::for_distro(&id).is_some();

    DistroInfo {
        id,
        name,
        version,
        version_id,
        upgrade_supported,
    }
}

#[cfg(test)]
mod tests {
    use super::{detect_distro, UpgradeStrategy};

    #[test]
    fn upgrade_strategy_covers_exactly_the_implemented_distros() {
        assert_eq!(
            UpgradeStrategy::for_distro("ubuntu"),
            Some(UpgradeStrategy::Ubuntu)
        );
        assert_eq!(
            UpgradeStrategy::for_distro("fedora"),
            Some(UpgradeStrategy::Fedora)
        );
        assert_eq!(
            UpgradeStrategy::for_distro("opensuse-leap"),
            Some(UpgradeStrategy::OpenSuseLeap)
        );
        assert_eq!(
            UpgradeStrategy::for_distro("nixos"),
            Some(UpgradeStrategy::NixOs)
        );
        // Previously claimed "supported" but never implemented — now honest.
        assert_eq!(UpgradeStrategy::for_distro("debian"), None);
        assert_eq!(UpgradeStrategy::for_distro("linuxmint"), None);
        assert_eq!(UpgradeStrategy::for_distro("centos"), None);
        assert_eq!(UpgradeStrategy::for_distro("arch"), None);
    }

    #[test]
    fn detect_distro_upgrade_supported_matches_strategy() {
        // Whatever this host is, the flag must agree with the strategy table.
        let info = detect_distro();
        assert_eq!(
            info.upgrade_supported,
            UpgradeStrategy::for_distro(&info.id).is_some()
        );
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
