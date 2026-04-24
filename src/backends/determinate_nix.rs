use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::runner::CommandRunner;
use std::future::Future;
use std::pin::Pin;

/// Returns true when Determinate Nix (by Determinate Systems) is installed.
///
/// Detection uses two markers in conjunction:
/// 1. `/nix/receipt.json` — created exclusively by the Determinate Nix installer.
///    This file is the canonical indicator and is NOT present in upstream Nix.
/// 2. `determinate-nixd` binary on PATH — confirms the daemon is installed and
///    the installation is complete (avoids false positives from partial installs).
pub fn is_available() -> bool {
    std::path::Path::new("/nix/receipt.json").exists() && which::which("determinate-nixd").is_ok()
}

/// Parse `determinate-nixd version` output to detect if an upgrade is available.
///
/// Returns `true` if the output contains the phrase "An upgrade is available"
/// (the canonical indicator from Determinate Systems documentation).
fn upgrade_available_in_output(output: &str) -> bool {
    output
        .lines()
        .any(|l| l.to_ascii_lowercase().contains("an upgrade is available"))
}

/// Parse upgraded/already-up-to-date from `determinate-nixd upgrade` output.
fn count_determinate_upgraded(output: &str) -> usize {
    let lower = output.to_ascii_lowercase();
    if lower.contains("nothing to upgrade")
        || lower.contains("already up to date")
        || lower.contains("already on the latest")
    {
        return 0;
    }
    if lower.contains("upgraded") || lower.contains("upgrading") || lower.contains("successfully") {
        return 1;
    }
    // Default: command succeeded, assume something changed
    1
}

pub struct DeterminateNixBackend;

impl Backend for DeterminateNixBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::DeterminateNix
    }

    fn display_name(&self) -> &str {
        "Determinate Nix"
    }

    fn description(&self) -> &str {
        "Determinate Nix installation (determinate-nixd)"
    }

    fn icon_name(&self) -> &str {
        "system-software-install-symbolic"
    }

    fn needs_root(&self) -> bool {
        true
    }

    fn run_update<'a>(
        &'a self,
        runner: &'a CommandRunner,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            // pkexec resets PATH; restore the Nix binary directory explicitly.
            // `determinate-nixd upgrade` upgrades the Determinate Nix installation
            // to the latest version advised by Determinate Systems.
            match runner
                .run(
                    "pkexec",
                    &[
                        "env",
                        "PATH=/nix/var/nix/profiles/default/bin:/run/wrappers/bin",
                        "sh",
                        "-c",
                        "determinate-nixd upgrade",
                    ],
                )
                .await
            {
                Ok(output) => UpdateResult::Success {
                    updated_count: count_determinate_upgraded(&output),
                },
                Err(e) => UpdateResult::Error(e),
            }
        })
    }

    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async move {
            // `determinate-nixd version` is unprivileged and reports whether
            // an upgrade is available.
            let out = tokio::process::Command::new("determinate-nixd")
                .arg("version")
                .output()
                .await
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let combined = format!("{text}\n{stderr}");
            if upgrade_available_in_output(&combined) {
                Ok(1)
            } else {
                Ok(0)
            }
        })
    }

    fn list_available(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("determinate-nixd")
                .arg("version")
                .output()
                .await
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let combined = format!("{text}\n{stderr}");
            if upgrade_available_in_output(&combined) {
                Ok(vec!["determinate-nix".to_string()])
            } else {
                Ok(Vec::new())
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{count_determinate_upgraded, upgrade_available_in_output};

    #[test]
    fn upgrade_available_in_output_detects_upgrade() {
        assert!(upgrade_available_in_output(
            "Determinate Nix v3.6.2\nAn upgrade is available: v3.7.0\nRun `sudo determinate-nixd upgrade` to upgrade."
        ));
    }

    #[test]
    fn upgrade_available_in_output_no_upgrade() {
        assert!(!upgrade_available_in_output("Determinate Nix v3.6.2\n"));
    }

    #[test]
    fn upgrade_available_in_output_case_insensitive() {
        assert!(upgrade_available_in_output(
            "AN UPGRADE IS AVAILABLE: v3.7.0"
        ));
    }

    #[test]
    fn count_determinate_upgraded_nothing_to_upgrade() {
        assert_eq!(count_determinate_upgraded("Nothing to upgrade"), 0);
    }

    #[test]
    fn count_determinate_upgraded_already_up_to_date() {
        assert_eq!(count_determinate_upgraded("Already up to date"), 0);
    }

    #[test]
    fn count_determinate_upgraded_already_on_latest() {
        assert_eq!(
            count_determinate_upgraded("Already on the latest version"),
            0
        );
    }

    #[test]
    fn count_determinate_upgraded_success() {
        assert_eq!(
            count_determinate_upgraded("Successfully upgraded to v3.7.0"),
            1
        );
    }

    #[test]
    fn count_determinate_upgraded_upgrading() {
        assert_eq!(count_determinate_upgraded("Upgrading from v3.6.2..."), 1);
    }

    #[test]
    fn count_determinate_upgraded_default() {
        assert_eq!(count_determinate_upgraded("Some unknown output"), 1);
    }
}
