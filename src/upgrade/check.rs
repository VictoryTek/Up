use super::detect::DistroInfo;
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub name: String,
    pub passed: bool,
    pub message: String,
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
    use crate::backends::os_package_manager::{
        parse_apt_list_upgradable, parse_dnf_list_upgrades, parse_zypper_list_updates,
    };

    // Reuse the backend parsers instead of a raw line count: `dnf check-update`
    // emits a "Last metadata expiration check" header and blank-separated
    // sections that a naive count mistakes for pending packages, which would
    // block the upgrade on an already-current Fedora system.
    let (cmd, args, count_pending): (&str, &[&str], fn(&str) -> usize) = match distro.id.as_str() {
        "ubuntu" => ("apt", &["list", "--upgradable"], |s| {
            parse_apt_list_upgradable(s).len()
        }),
        "fedora" => ("dnf", &["check-update"], |s| {
            parse_dnf_list_upgrades(s).len()
        }),
        "opensuse-leap" => ("zypper", &["list-updates"], |s| {
            parse_zypper_list_updates(s).len()
        }),
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

    match Command::new(cmd)
        .args(args)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .output()
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let upgradable = count_pending(&stdout);

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

pub(crate) fn parse_df_avail_bytes(stdout: &str) -> Result<u64, String> {
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

#[cfg(test)]
mod tests {
    use super::parse_df_avail_bytes;
    use crate::backends::os_package_manager::parse_dnf_list_upgrades;

    #[test]
    fn dnf_metadata_only_output_counts_as_zero_pending() {
        // Regression for BUGS M6: the "Last metadata expiration check" header
        // and section labels must not be counted as pending packages.
        let stdout = "Last metadata expiration check: 0:12:03 ago on Tue.\n\nObsoleting Packages\n";
        assert_eq!(parse_dnf_list_upgrades(stdout).len(), 0);
    }

    #[test]
    fn dnf_real_updates_are_counted() {
        let stdout = "Last metadata expiration check: 0:00:10 ago.\n\
             bash.x86_64   5.2.26-1.fc40   updates\n\
             curl.x86_64   8.6.0-8.fc40    updates\n";
        assert_eq!(parse_dnf_list_upgrades(stdout).len(), 2);
    }

    #[test]
    fn parse_df_avail_bytes_normal() {
        let output = "     Avail\n10737418240\n";
        assert_eq!(parse_df_avail_bytes(output), Ok(10_737_418_240u64));
    }

    #[test]
    fn parse_df_avail_bytes_genuine_zero() {
        let output = "     Avail\n0\n";
        assert_eq!(parse_df_avail_bytes(output), Ok(0u64));
    }

    #[test]
    fn parse_df_avail_bytes_empty_stdout() {
        assert!(parse_df_avail_bytes("").is_err());
    }

    #[test]
    fn parse_df_avail_bytes_header_only() {
        let output = "     Avail\n";
        assert!(parse_df_avail_bytes(output).is_err());
    }

    #[test]
    fn parse_df_avail_bytes_non_numeric() {
        let output = "     Avail\nN/A\n";
        assert!(parse_df_avail_bytes(output).is_err());
    }

    #[test]
    fn parse_df_avail_bytes_locale_comma() {
        let output = "     Avail\n10,737,418,240\n";
        assert!(parse_df_avail_bytes(output).is_err());
    }
}
