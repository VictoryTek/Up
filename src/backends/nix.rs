use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::runner::CommandRunner;

pub fn is_available() -> bool {
    which::which("nix").is_ok()
}

/// True when running on NixOS (the /etc/nixos directory is present).
fn is_nixos() -> bool {
    std::path::Path::new("/etc/nixos").exists()
}

/// True when the NixOS config is flake-based (/etc/nixos/flake.nix exists).
fn is_nixos_flake() -> bool {
    std::path::Path::new("/etc/nixos/flake.nix").exists()
}

fn nixos_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .unwrap_or_else(|_| "nixos".to_owned())
        .trim()
        .to_string()
}

/// Validates that a hostname is safe to use as a NixOS flake output attribute.
/// Only ASCII alphanumeric, hyphen, and underscore are permitted.
fn validate_hostname(hostname: &str) -> Result<&str, String> {
    if hostname.is_empty() {
        return Err("hostname is empty".to_string());
    }
    if hostname.len() > 63 {
        return Err(format!(
            "hostname is too long ({} chars, max 63)",
            hostname.len()
        ));
    }
    if !hostname
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("Invalid hostname: {:?}", hostname));
    }
    Ok(hostname)
}

pub struct NixBackend;

#[async_trait::async_trait]
impl Backend for NixBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Nix
    }

    fn display_name(&self) -> &str {
        "Nix"
    }

    fn description(&self) -> &str {
        if is_nixos() {
            "NixOS system packages"
        } else {
            "Nix profile packages"
        }
    }

    fn icon_name(&self) -> &str {
        "system-software-install-symbolic"
    }

    async fn run_update(&self, runner: &CommandRunner) -> UpdateResult {
        if is_nixos() {
            if is_nixos_flake() {
                // Flake-based NixOS: update the flake inputs then rebuild.
                // Validate the hostname before use to prevent shell injection.
                let raw_hostname = nixos_hostname();
                let hostname = match validate_hostname(&raw_hostname) {
                    Ok(h) => h,
                    Err(e) => return UpdateResult::Error(e),
                };
                // Export the NixOS binary paths explicitly: pkexec resets PATH
                // to standard directories that typically do not include Nix
                // tooling on NixOS. Use two separate runner.run() calls instead
                // of sh -c to avoid shell injection.
                //
                // Call 1: update flake inputs, passing /etc/nixos as an argument.
                if let Err(e) = runner
                    .run(
                        "pkexec",
                        &[
                            "env",
                            "PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin",
                            "nix",
                            "--extra-experimental-features",
                            "nix-command flakes",
                            "flake",
                            "update",
                            "/etc/nixos",
                        ],
                    )
                    .await
                {
                    return UpdateResult::Error(e);
                }
                // Call 2: rebuild the system with the validated flake path.
                let flake_arg = format!("/etc/nixos#{}", hostname);
                match runner
                    .run(
                        "pkexec",
                        &[
                            "env",
                            "PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin",
                            "nixos-rebuild",
                            "switch",
                            "--flake",
                            &flake_arg,
                        ],
                    )
                    .await
                {
                    Ok(output) => UpdateResult::Success {
                        updated_count: output.lines().filter(|l| !l.is_empty()).count(),
                    },
                    Err(e) => UpdateResult::Error(e),
                }
            } else {
                // Legacy NixOS channels.
                match runner
                    .run("pkexec", &["nixos-rebuild", "switch", "--upgrade"])
                    .await
                {
                    Ok(output) => UpdateResult::Success {
                        updated_count: output.lines().filter(|l| !l.is_empty()).count(),
                    },
                    Err(e) => UpdateResult::Error(e),
                }
            }
        } else {
            // Non-NixOS: update the user's nix profile.
            // Detect whether the user's nix profile uses the flake/v2 manifest format.
            // This is a silent filesystem check — it does NOT use runner.run() and
            // therefore does not emit any log output before the real update starts.
            let use_flakes = {
                let manifest_path =
                    std::path::Path::new(&std::env::var("HOME").unwrap_or_default())
                        .join(".nix-profile/manifest.json");
                if let Ok(content) = std::fs::read_to_string(&manifest_path) {
                    // Flake profiles have "version": 2 in their manifest
                    content.contains("\"version\": 2")
                } else {
                    // If we can't read the manifest, fall back to the legacy nix-env path
                    false
                }
            };
            if use_flakes {
                match runner.run("nix", &["profile", "upgrade", ".*"]).await {
                    Ok(output) => UpdateResult::Success {
                        updated_count: output.lines().filter(|l| !l.is_empty()).count(),
                    },
                    Err(e) => UpdateResult::Error(e),
                }
            } else {
                match runner.run("nix-env", &["-u"]).await {
                    Ok(output) => UpdateResult::Success {
                        updated_count: output.lines().filter(|l| l.contains("upgrading")).count(),
                    },
                    Err(e) => UpdateResult::Error(e),
                }
            }
        }
    }

    async fn count_available(&self) -> Result<usize, String> {
        if is_nixos() {
            if is_nixos_flake() {
                // For flake-based NixOS: parse the lock file to count tracked inputs.
                // This is read-only, requires no network, and has no side effects.
                // We report the number of locked inputs as an informational count —
                // a full freshness check would require a network fetch which belongs
                // only in run_update.
                let lock_content = tokio::fs::read_to_string("/etc/nixos/flake.lock")
                    .await
                    .map_err(|e| format!("Cannot read /etc/nixos/flake.lock: {e}"))?;
                let lock: serde_json::Value = serde_json::from_str(&lock_content)
                    .map_err(|e| format!("Cannot parse flake.lock: {e}"))?;
                let count = lock
                    .get("nodes")
                    .and_then(|n| n.as_object())
                    .map(|nodes| {
                        nodes
                            .values()
                            .filter(|v| v.get("locked").is_some())
                            .count()
                    })
                    .unwrap_or(0);
                Ok(count)
            } else {
                // Legacy NixOS channels have no unprivileged check mechanism.
                Err("Click Update All to check".to_string())
            }
        } else {
            let out = tokio::process::Command::new("nix-env")
                .args(["-u", "--dry-run"])
                .output()
                .await
                .map_err(|e| e.to_string())?;
            // nix-env --dry-run writes "upgrading ..." lines to stderr
            let text = String::from_utf8_lossy(&out.stderr);
            Ok(text.lines().filter(|l| l.contains("upgrading")).count())
        }
    }
}
