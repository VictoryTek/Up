use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::runner::CommandRunner;
use std::future::Future;
use std::pin::Pin;

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
/// NixOS does not record the flake attribute name in any standard runtime file —
/// the store path uses `networking.hostName`, not the flake attr, and
/// `nixos-rebuild` itself falls back to the hostname when no attribute is given.
///
/// Resolution order:
///
/// 1. `/etc/nixos/up-flake-attr` — a user-maintained file containing exactly
///    the flake attribute name (e.g. "vexos-nvidia"). Recommended for systems
///    where multiple configurations share the same `networking.hostName`.
///    Create it once with: `sudo sh -c 'echo vexos-nvidia > /etc/nixos/up-flake-attr'`
///
/// 2. Parse the running system's Nix store path from `/run/current-system`.
///    The format is `nixos-system-{networking.hostName}-{nixos-version}`.
///    If the extracted name exactly matches a `nixosConfigurations` attribute,
///    it is used. This works when each config has a distinct `networking.hostName`
///    that matches its flake attribute name.
///
/// 3. Return a descriptive error listing all available configurations and
///    instructions for creating the config file.
///
/// 4. Last resort: fall back to the raw hostname when `nix eval` is unavailable.
fn resolve_nixos_flake_attr() -> Result<String, String> {
    const CONFIG_FILE: &str = "/etc/nixos/up-flake-attr";

    // Step 1: User-maintained explicit override file.
    if let Ok(content) = std::fs::read_to_string(CONFIG_FILE) {
        let name = content.trim().to_string();
        if !name.is_empty() {
            return validate_flake_attr(&name);
        }
    }

    // Step 2: List available nixosConfigurations from the flake.
    // Unprivileged read-only operation; runs as the current user.
    let available_names: Option<Vec<String>> = (|| {
        let out = std::process::Command::new("nix")
            .args([
                "--extra-experimental-features",
                "nix-command flakes",
                "eval",
                "--json",
                "--no-write-lock-file",
                "/etc/nixos#nixosConfigurations",
                "--apply",
                "builtins.attrNames",
            ])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let stdout = std::str::from_utf8(&out.stdout).ok()?;
        serde_json::from_str(stdout.trim()).ok()
    })();

    // Step 3: Parse the running system name from the /run/current-system symlink.
    // Target format: /nix/store/<HASH>-nixos-system-<name>-<nixos-version>
    // <name> is networking.hostName. Strip the store prefix and version suffix
    // to recover it.
    let system_name: Option<String> = (|| {
        let link = std::fs::read_link("/run/current-system").ok()?;
        let basename = link.file_name()?.to_str()?;
        let rest = basename.strip_prefix("nixos-system-")?;
        // The NixOS version suffix is a hyphen-separated component that starts
        // with a digit (e.g. "24.05.1234.abcdef1"). Walk from the right,
        // removing such components.
        let mut parts: Vec<&str> = rest.split('-').collect();
        while let Some(last) = parts.last() {
            if last.chars().next().map_or(false, |c| c.is_ascii_digit()) {
                parts.pop();
            } else {
                break;
            }
        }
        if parts.is_empty() {
            return None;
        }
        Some(parts.join("-"))
    })();

    // Step 4: Cross-reference the running system name with flake attribute names.
    if let (Some(names), Some(sys_name)) = (&available_names, &system_name) {
        if names.contains(sys_name) {
            // Exact match: hostName == flake attr — no ambiguity.
            return validate_flake_attr(sys_name);
        }
        // The running system's hostName does not match any flake attribute.
        // This is typical when all configurations share the same networking.hostName
        // (e.g. "vexos") but have distinct attribute names (e.g. "vexos-nvidia").
        // There is no standard NixOS mechanism that records which attribute was used
        // to build the running system, so we cannot determine it automatically.
        return Err(format!(
            "Cannot determine the active NixOS configuration automatically. \
             The running system is '{}', but that does not match any available \
             configuration: {}. \
             Create /etc/nixos/up-flake-attr containing the correct name, e.g.: \
             sudo sh -c 'echo vexos-nvidia > /etc/nixos/up-flake-attr'",
            sys_name,
            names.join(", "),
        ));
    }

    // Step 5: nix eval unavailable — fall back to raw hostname.
    validate_flake_attr(&nixos_hostname())
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

    fn run_update<'a>(&'a self, runner: &'a CommandRunner) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
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
                            updated_count: output.lines().filter(|l| !l.is_empty()).count(),
                        },
                        Err(e) => UpdateResult::Error(e),
                    }
                } else {
                    // Legacy NixOS channels: update channel metadata first,
                    // then rebuild the system.
                    if let Err(e) = runner
                        .run("pkexec", &["nix-channel", "--update"])
                        .await
                    {
                        return UpdateResult::Error(e);
                    }
                    match runner
                        .run("pkexec", &["nixos-rebuild", "switch"])
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
        })
    }

    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async move {
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
        })
    }
}
