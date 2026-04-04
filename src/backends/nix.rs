use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::runner::CommandRunner;
use std::future::Future;
use std::pin::Pin;

pub fn is_available() -> bool {
    which::which("nix").is_ok()
}

/// True when running on NixOS.
///
/// Uses multiple indicators in order of reliability:
/// 1. `/run/current-system` — NixOS-specific symlink created by the activation
///    script; present on every running NixOS system regardless of config location.
/// 2. `ID=nixos` in `/etc/os-release` — standard OS identifier.
/// 3. `/etc/nixos` — legacy fallback for traditional config locations.
fn is_nixos() -> bool {
    if std::path::Path::new("/run/current-system").exists() {
        return true;
    }
    if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
        if content.lines().any(|l| l.trim() == "ID=nixos") {
            return true;
        }
    }
    std::path::Path::new("/etc/nixos").exists()
}

/// True when the NixOS config is flake-based (/etc/nixos/flake.nix exists).
fn is_nixos_flake() -> bool {
    std::path::Path::new("/etc/nixos/flake.nix").exists()
}

/// Validates that a string is safe to use as a NixOS flake output attribute.
/// Only ASCII alphanumeric, hyphen, underscore, and dot are permitted.
fn validate_flake_attr(name: &str) -> Result<String, String> {
    if name.is_empty() {
        return Err("flake attribute name is empty".to_string());
    }
    if name.len() > 253 {
        return Err(format!(
            "flake attribute name is too long ({} chars, max 253)",
            name.len()
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(format!("Invalid flake attribute name: {:?}", name));
    }
    Ok(name.to_string())
}

/// Determine the NixOS configuration attribute name to use for flake rebuilds.
///
/// Resolution order:
///
/// 1. `/etc/nixos/vexos-variant` — a user-maintained file containing exactly
///    the flake attribute name (e.g. "vexos-nvidia"). Created by the VexOS
///    NixOS configuration to track which variant is installed on the system.
///    Example: `sudo sh -c 'echo vexos-nvidia > /etc/nixos/vexos-variant'`
///
/// 2. Return an error with instructions for creating the file.
fn resolve_nixos_flake_attr() -> Result<String, String> {
    const VARIANT_FILE: &str = "/etc/nixos/vexos-variant";

    // Step 1: Read the variant file (mandatory, primary source of truth).
    match std::fs::read_to_string(VARIANT_FILE) {
        Ok(content) => {
            let variant = content.trim().to_string();
            if variant.is_empty() {
                return Err("Variant file /etc/nixos/vexos-variant is empty".to_string());
            }
            // Validate and return the variant name
            validate_flake_attr(&variant)
        }
        Err(e) => Err(format!(
            "Cannot read {}: {}. This file must exist and contain the flake attribute name. \
             If this is a VexOS system, ensure the variant file was created during system configuration.",
            VARIANT_FILE, e
        )),
    }
}

pub struct NixBackend;

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

    fn run_update<'a>(
        &'a self,
        runner: &'a CommandRunner,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            if is_nixos() {
                if is_nixos_flake() {
                    // Flake-based NixOS: update the flake inputs then rebuild.
                    //
                    // Resolve the actual nixosConfigurations attribute name from the
                    // flake — the hostname alone may not match (e.g. hostname "vexos"
                    // but configs are "vexos-nvidia", "vexos-intel", "vexos-vm").
                    let config_name = match resolve_nixos_flake_attr() {
                        Ok(n) => n,
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
                                "--flake",
                                "/etc/nixos",
                            ],
                        )
                        .await
                    {
                        return UpdateResult::Error(e);
                    }
                    // Call 2: rebuild the system with the resolved configuration name.
                    let flake_arg = format!("/etc/nixos#{}", config_name);
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
                            updated_count: output
                                .lines()
                                .filter(|l| !l.is_empty())
                                .count(),
                        },
                        Err(e) => UpdateResult::Error(e),
                    }
                } else {
                    // Legacy NixOS channels: update channel metadata first,
                    // then rebuild the system.
                    if let Err(e) = runner.run("pkexec", &["nix-channel", "--update"]).await {
                        return UpdateResult::Error(e);
                    }
                    match runner.run("pkexec", &["nixos-rebuild", "switch"]).await {
                        Ok(output) => UpdateResult::Success {
                            updated_count: output
                                .lines()
                                .filter(|l| l.contains("upgrading"))
                                .count(),
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
                            updated_count: output
                                .lines()
                                .filter(|l| l.contains("upgrading"))
                                .count(),
                        },
                        Err(e) => UpdateResult::Error(e),
                    }
                }
            }
        })
    }

    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async move {
            if is_nixos() {
                // NixOS (both flake and legacy channel) cannot determine whether
                // updates are available without a network fetch, which belongs only
                // in run_update. Reporting the number of locked flake inputs is
                // misleading — those are pinned dependencies, not pending upgrades,
                // and the count never changes after running an update.
                Err("Run Update All to check".to_string())
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
        })
    }
}
