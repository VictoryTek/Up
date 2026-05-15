use crate::backends::{Backend, BackendError, BackendKind, UpdateResult};
use crate::executor::CommandExecutor;
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
    if crate::backends::flatpak::is_running_in_flatpak() {
        // Inside the Flatpak sandbox, probe the host filesystem via flatpak-spawn.
        // /run/current-system is the most reliable NixOS-specific indicator.
        return std::process::Command::new("flatpak-spawn")
            .args(["--host", "test", "-e", "/run/current-system"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    }
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
    if crate::backends::flatpak::is_running_in_flatpak() {
        return std::process::Command::new("flatpak-spawn")
            .args(["--host", "test", "-e", "/etc/nixos/flake.nix"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    }
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
pub(crate) fn resolve_nixos_flake_attr() -> Result<String, String> {
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

/// Parse the combined stdout+stderr output of Nix build commands to count how
/// many store paths were actually built or fetched.
///
/// Nix emits lines of the form:
///   "these N derivations will be built:"
///   "these N paths will be fetched (X MiB download, Y MiB unpacked):"
///
/// These lines are only present when real work is done. When the system is
/// already up to date they are absent, so this function correctly returns 0.
pub(crate) fn count_nix_store_operations(output: &str) -> usize {
    let mut total = 0usize;
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("these ")
            && (trimmed.contains("derivations will be built")
                || trimmed.contains("paths will be fetched"))
        {
            let after_these = &trimmed["these ".len()..];
            if let Some(n_str) = after_these.split_whitespace().next() {
                total += n_str.parse::<usize>().unwrap_or(0);
            }
        }
    }
    total
}

/// Compare two flake.lock JSON values and return the names of inputs whose
/// locked revision or `lastModified` timestamp changed between the two files.
/// New inputs (present in `new` but absent in `old`) are also reported.
pub(crate) fn compare_lock_nodes(old: &serde_json::Value, new: &serde_json::Value) -> Vec<String> {
    let old_nodes = old["nodes"].as_object();
    let new_nodes = new["nodes"].as_object();
    let mut changed = Vec::new();
    if let (Some(old_map), Some(new_map)) = (old_nodes, new_nodes) {
        for (name, new_val) in new_map {
            if name == "root" {
                continue;
            }
            if let Some(old_val) = old_map.get(name) {
                let old_rev = &old_val["locked"]["rev"];
                let new_rev = &new_val["locked"]["rev"];
                let old_mod = &old_val["locked"]["lastModified"];
                let new_mod = &new_val["locked"]["lastModified"];
                if old_rev != new_rev || old_mod != new_mod {
                    changed.push(name.clone());
                }
            } else {
                // Input newly added in the updated lock.
                changed.push(name.clone());
            }
        }
    }
    changed
}

/// Try `nix flake update --dry-run /etc/nixos` (Nix ≥ 2.19).
///
/// Returns:
/// - `Ok(Some(inputs))` – success; `inputs` is the list of changed input names.
/// - `Ok(None)`         – `--dry-run` flag is not recognised; caller should fall back.
/// - `Err(msg)`         – a real (non-flag-support) error occurred.
async fn nixos_flake_dry_run_check() -> Result<Option<Vec<String>>, String> {
    let out = tokio::process::Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "--option",
            "eval-cache",
            "false",
            "--option",
            "tarball-ttl",
            "0",
            "flake",
            "update",
            "--dry-run",
            "/etc/nixos",
        ])
        .output()
        .await
        .map_err(|e| e.to_string())?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}\n{stderr}");

    // Detect unsupported --dry-run flag (older Nix) — signal caller to fall back.
    if !out.status.success()
        && (combined.contains("unrecognised flag")
            || combined.contains("unrecognized flag")
            || combined.contains("unknown option"))
    {
        return Ok(None);
    }

    if !out.status.success() {
        return Err(format!(
            "nix flake update --dry-run failed: {}",
            combined.trim()
        ));
    }

    // Parse lines like: "• Updated input 'nixpkgs':"
    let inputs: Vec<String> = combined
        .lines()
        .filter(|l| l.trim_start().starts_with("\u{2022} Updated input '"))
        .filter_map(|l| l.split('\'').nth(1).map(|s| s.to_string()))
        .collect();

    Ok(Some(inputs))
}

/// Fallback flake check that works on all Nix versions.
///
/// Copies `/etc/nixos/flake.nix` and `/etc/nixos/flake.lock` into a
/// temporary directory, runs `nix flake update` there (no root required),
/// and compares the resulting `flake.lock` against the original to determine
/// which inputs changed.  The temp directory is removed after comparison.
async fn nixos_flake_tempdir_check() -> Result<Vec<String>, String> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let temp_dir = std::env::temp_dir().join(format!("up-nix-check-{ts}"));

    std::fs::create_dir_all(&temp_dir).map_err(|e| format!("Failed to create temp dir: {e}"))?;

    // Copy both files; clean up on any early error.
    let cleanup = |e: String| {
        let _ = std::fs::remove_dir_all(&temp_dir);
        e
    };

    std::fs::copy("/etc/nixos/flake.nix", temp_dir.join("flake.nix"))
        .map_err(|e| cleanup(format!("Failed to copy flake.nix: {e}")))?;
    std::fs::copy("/etc/nixos/flake.lock", temp_dir.join("flake.lock"))
        .map_err(|e| cleanup(format!("Failed to copy flake.lock: {e}")))?;

    // Read and parse original lock before running the update.
    let old_content = std::fs::read_to_string(temp_dir.join("flake.lock"))
        .map_err(|e| cleanup(format!("Failed to read flake.lock: {e}")))?;
    let old_lock: serde_json::Value = serde_json::from_str(&old_content)
        .map_err(|e| cleanup(format!("Failed to parse flake.lock: {e}")))?;

    // Run `nix flake update` from inside the temp dir — writes updated flake.lock in-place.
    // Passing the path as a positional argument tells Nix to update a *named input*, not the
    // flake directory; using `.current_dir()` is the correct cross-version approach.
    let out = tokio::process::Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "--option",
            "eval-cache",
            "false",
            "--option",
            "tarball-ttl",
            "0",
            "flake",
            "update",
        ])
        .current_dir(&temp_dir)
        .output()
        .await
        .map_err(|e| cleanup(e.to_string()))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(cleanup(format!(
            "nix flake update (temp dir) failed: {}",
            stderr.trim()
        )));
    }

    let new_content = std::fs::read_to_string(temp_dir.join("flake.lock"))
        .map_err(|e| cleanup(format!("Failed to read updated flake.lock: {e}")))?;
    let new_lock: serde_json::Value = serde_json::from_str(&new_content)
        .map_err(|e| cleanup(format!("Failed to parse updated flake.lock: {e}")))?;

    let _ = std::fs::remove_dir_all(&temp_dir);
    Ok(compare_lock_nodes(&old_lock, &new_lock))
}

/// Return the list of NixOS flake inputs that have pending upstream updates.
///
/// Tries `--dry-run` first (Nix ≥ 2.19); falls back to the temp-dir method
/// for older Nix installations that do not support that flag.
async fn nixos_flake_changed_inputs() -> Result<Vec<String>, String> {
    match nixos_flake_dry_run_check().await? {
        Some(inputs) => Ok(inputs),
        None => nixos_flake_tempdir_check().await,
    }
}

/// True when Determinate Nix (by Determinate Systems) is installed.
///
/// Uses two markers in conjunction:
/// 1. `/nix/receipt.json` — created exclusively by the Determinate Nix installer.
/// 2. `determinate-nixd` binary on PATH — confirms the daemon is installed.
fn is_determinate_nix() -> bool {
    if crate::backends::flatpak::is_running_in_flatpak() {
        let receipt_ok = std::process::Command::new("flatpak-spawn")
            .args(["--host", "test", "-e", "/nix/receipt.json"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let daemon_ok = std::process::Command::new("flatpak-spawn")
            .args(["--host", "which", "determinate-nixd"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        return receipt_ok && daemon_ok;
    }
    std::path::Path::new("/nix/receipt.json").exists() && which::which("determinate-nixd").is_ok()
}

/// Parse `determinate-nixd version` output to detect if an upgrade is available.
///
/// Returns `true` if the output contains the phrase "An upgrade is available".
pub(crate) fn upgrade_available_in_output(output: &str) -> bool {
    output
        .lines()
        .any(|l| l.to_ascii_lowercase().contains("an upgrade is available"))
}

/// Run `nix profile upgrade` in a way that works across Nix versions.
///
/// Nix ≥ 2.18 uses `--all`; older versions used a regex argument (`.*`).
/// We try `--all` first and silently fall back to `.*` when Nix complains
/// about an unrecognised option.
async fn nix_profile_upgrade_all() -> Result<String, String> {
    let new_style = tokio::process::Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command",
            "profile",
            "upgrade",
            "--all",
        ])
        .output()
        .await
        .map_err(|e| e.to_string())?;

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&new_style.stdout),
        String::from_utf8_lossy(&new_style.stderr)
    );

    if new_style.status.success() {
        return Ok(combined);
    }

    // Detect unsupported --all flag (Nix < 2.18) — fall back to regex syntax.
    let unrecognised = combined.contains("unrecognised flag")
        || combined.contains("unrecognized flag")
        || combined.contains("unknown option")
        || combined.contains("unexpected argument");

    if !unrecognised {
        return Err(format!(
            "nix profile upgrade --all failed: {}",
            combined.trim()
        ));
    }

    // Legacy: `nix profile upgrade '.*'`
    let old_style = tokio::process::Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command",
            "profile",
            "upgrade",
            ".*",
        ])
        .output()
        .await
        .map_err(|e| e.to_string())?;

    let out = format!(
        "{}\n{}",
        String::from_utf8_lossy(&old_style.stdout),
        String::from_utf8_lossy(&old_style.stderr)
    );

    if old_style.status.success() {
        Ok(out)
    } else {
        Err(format!("nix profile upgrade failed: {}", out.trim()))
    }
}

/// Parse upgraded/already-up-to-date status from `determinate-nixd upgrade` output.
pub(crate) fn count_determinate_upgraded(output: &str) -> usize {
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

pub struct NixBackend;

impl Backend for NixBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Nix
    }

    fn display_name(&self) -> &str {
        "Nix"
    }

    fn description(&self) -> &str {
        if is_determinate_nix() {
            "Determinate Nix installation (determinate-nixd)"
        } else if is_nixos() {
            "NixOS system packages"
        } else {
            "Nix profile packages"
        }
    }

    fn icon_name(&self) -> &str {
        "system-software-install-symbolic"
    }

    fn needs_root(&self) -> bool {
        // NixOS rebuilds require root. Determinate Nix `upgrade` also requires
        // root (it upgrades the Nix installation, not just user packages).
        is_nixos() || is_determinate_nix()
    }

    fn run_update<'a>(
        &'a self,
        runner: &'a dyn CommandExecutor,
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
                        Err(e) => return UpdateResult::Error(BackendError::from_string(e)),
                    };
                    // Single pkexec invocation so polkit only prompts once.
                    // pkexec resets PATH, so we restore the NixOS binary paths
                    // explicitly via `env PATH=...` before invoking sh.
                    // config_name is validated by validate_flake_attr (ASCII
                    // alphanumeric / hyphen / underscore / dot only), so it is
                    // safe to interpolate into the shell command string.
                    let cmd = format!(
                        "stdbuf -oL -eL \
                         nix --extra-experimental-features 'nix-command flakes' \
                         flake update --flake /etc/nixos && \
                         stdbuf -oL -eL \
                         nixos-rebuild switch --flake /etc/nixos#{} --refresh --print-build-logs",
                        config_name
                    );
                    match runner
                        .run(
                            "pkexec",
                            &[
                                "env",
                                "PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin",
                                "sh",
                                "-c",
                                &cmd,
                            ],
                        )
                        .await
                    {
                        Ok(output) => UpdateResult::Success {
                            updated_count: count_nix_store_operations(&output),
                        },
                        Err(e) => UpdateResult::Error(e),
                    }
                } else {
                    // Legacy NixOS channels: single pkexec so polkit only prompts once.
                    match runner
                        .run(
                            "pkexec",
                            &[
                                "env",
                                "PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin",
                                "sh",
                                "-c",
                                "stdbuf -oL -eL nix-channel --update && \
                                 stdbuf -oL -eL nixos-rebuild switch --print-build-logs",
                            ],
                        )
                        .await {
                        Ok(output) => UpdateResult::Success {
                            updated_count: count_nix_store_operations(&output),
                        },
                        Err(e) => UpdateResult::Error(e),
                    }
                }
            } else {
                // Non-NixOS: update the user's nix profile.
                //
                // Check for Determinate Nix first — it runs unprivileged via its daemon.
                if is_determinate_nix() {
                    // `determinate-nixd upgrade` upgrades the Nix installation itself and
                    // requires root. pkexec resets PATH so we must pass the resolved absolute
                    // path to the binary — otherwise pkexec cannot find it.
                    let nixd_path = match which::which("determinate-nixd") {
                        Ok(p) => p.to_string_lossy().to_string(),
                        Err(_) => {
                            return UpdateResult::Error(BackendError::Spawn(
                                "determinate-nixd not found on PATH".to_string(),
                            ))
                        }
                    };
                    match runner.run("pkexec", &[nixd_path.as_str(), "upgrade"]).await {
                        Ok(output) => UpdateResult::Success {
                            updated_count: count_determinate_upgraded(&output),
                        },
                        Err(e) => UpdateResult::Error(e),
                    }
                } else {
                    // Detect whether the user's nix profile uses the legacy v1 manifest format.
                    // This is a silent filesystem check — it does NOT use runner.run() and
                    // therefore does not emit any log output before the real update starts.
                    let use_legacy_nix_env = {
                        let manifest_path =
                            std::path::Path::new(&std::env::var("HOME").unwrap_or_default())
                                .join(".nix-profile/manifest.json");
                        if let Ok(content) = std::fs::read_to_string(&manifest_path) {
                            // Only use nix-env when manifest is NOT version 2
                            !content.contains("\"version\": 2")
                        } else {
                            // Can't read manifest — default to modern nix profile upgrade
                            false
                        }
                    };
                    if use_legacy_nix_env {
                        match runner.run("nix-env", &["-u"]).await {
                            Ok(output) => UpdateResult::Success {
                                updated_count: output
                                    .lines()
                                    .filter(|l| l.contains("upgrading"))
                                    .count(),
                            },
                            Err(e) => UpdateResult::Error(e),
                        }
                    } else {
                        // Use the version-aware helper: tries `--all` (Nix ≥ 2.18)
                        // then falls back to the legacy `.*` regex argument.
                        match nix_profile_upgrade_all().await {
                            Ok(output) => UpdateResult::Success {
                                updated_count: count_nix_store_operations(&output),
                            },
                            Err(e) => UpdateResult::Error(BackendError::from_string(e)),
                        }
                    }
                }
            }
        })
    }

    fn list_available(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
        Box::pin(async move {
            if is_nixos() && is_nixos_flake() {
                // Flake-based NixOS: return changed input names.
                nixos_flake_changed_inputs().await
            } else if is_determinate_nix() {
                // Determinate Nix: check version output for upgrade availability.
                let out = tokio::process::Command::new("determinate-nixd")
                    .arg("version")
                    .output()
                    .await
                    .map_err(|e| e.to_string())?;
                let text = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                let combined = format!("{text}\n{stderr}");
                Ok(if upgrade_available_in_output(&combined) {
                    vec!["determinate-nix".to_string()]
                } else {
                    Vec::new()
                })
            } else {
                // Non-NixOS Nix profile: check manifest version.
                let use_legacy_nix_env = {
                    let manifest_path =
                        std::path::Path::new(&std::env::var("HOME").unwrap_or_default())
                            .join(".nix-profile/manifest.json");
                    if let Ok(content) = std::fs::read_to_string(&manifest_path) {
                        !content.contains("\"version\": 2")
                    } else {
                        false
                    }
                };
                if use_legacy_nix_env {
                    let out = tokio::process::Command::new("nix-env")
                        .args(["-u", "--dry-run"])
                        .output()
                        .await
                        .map_err(|e| e.to_string())?;
                    // nix-env --dry-run emits "upgrading 'name-1.0' to 'name-2.0'" on stderr
                    let text = String::from_utf8_lossy(&out.stderr);
                    Ok(text
                        .lines()
                        .filter(|l| l.contains("upgrading"))
                        .filter_map(|l| l.split('\'').nth(1).map(|s| s.to_string()))
                        .collect())
                } else {
                    // nix profile upgrade has no dry-run equivalent
                    Ok(Vec::new())
                }
            }
        })
    }

    fn supports_cleanup(&self) -> bool {
        true
    }

    fn run_cleanup<'a>(
        &'a self,
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            // nix-collect-garbage -d deletes old profile generations and
            // collects unreachable store paths. Runs unprivileged on user profiles.
            match runner.run("nix-collect-garbage", &["-d"]).await {
                Ok(output) => {
                    let freed = count_nix_freed_paths(&output);
                    UpdateResult::Success {
                        updated_count: freed,
                    }
                }
                Err(e) => UpdateResult::Error(e),
            }
        })
    }

    fn supports_item_selection(&self) -> bool {
        is_nixos() && is_nixos_flake()
    }

    fn run_selected_update<'a>(
        &'a self,
        items: &'a [String],
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            // Validate all input names against the same rules as flake attributes.
            for input in items {
                if let Err(e) = validate_flake_attr(input) {
                    return UpdateResult::Error(BackendError::from_string(e));
                }
            }
            let config_name = match resolve_nixos_flake_attr() {
                Ok(n) => n,
                Err(e) => return UpdateResult::Error(BackendError::from_string(e)),
            };
            // Build: nix flake update <input1> <input2> ... --flake /etc/nixos
            // Then: nixos-rebuild switch --flake /etc/nixos#<config>
            // All inputs have been validated by validate_flake_attr above (ASCII
            // alphanumeric / hyphen / underscore / dot only), so interpolation is safe.
            let inputs_str = items.join(" ");
            let cmd = format!(
                "stdbuf -oL -eL \
                 nix --extra-experimental-features 'nix-command flakes' \
                 flake update {} --flake /etc/nixos && \
                 stdbuf -oL -eL \
                 nixos-rebuild switch --flake /etc/nixos#{} --refresh --print-build-logs",
                inputs_str, config_name
            );
            match runner
                .run(
                    "pkexec",
                    &[
                        "env",
                        "PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin",
                        "sh",
                        "-c",
                        &cmd,
                    ],
                )
                .await
            {
                Ok(output) => UpdateResult::Success {
                    updated_count: count_nix_store_operations(&output),
                },
                Err(e) => UpdateResult::Error(e),
            }
        })
    }
}

/// Count store paths freed by `nix-collect-garbage -d`.
/// Output contains lines like: "1234 store paths deleted, 567.89 MiB freed"
pub(crate) fn count_nix_freed_paths(output: &str) -> usize {
    for line in output.lines() {
        if line.contains("store paths deleted") {
            if let Some(n_str) = line.split_whitespace().next() {
                return n_str.parse::<usize>().unwrap_or(0);
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::{
        compare_lock_nodes, count_determinate_upgraded, count_nix_store_operations,
        is_determinate_nix, is_nixos, upgrade_available_in_output, validate_flake_attr, NixBackend,
    };
    use crate::backends::{Backend, UpdateResult};
    use crate::executor::test_utils::MockExecutor;

    /// Serialises all tests that mutate the HOME environment variable to prevent
    /// a race condition where parallel threads read the wrong HOME value.
    static HOME_ENV_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

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
    fn count_determinate_upgraded_nothing_to_upgrade() {
        assert_eq!(count_determinate_upgraded("nothing to upgrade\n"), 0);
    }

    #[test]
    fn count_determinate_upgraded_success() {
        assert_eq!(
            count_determinate_upgraded("Successfully upgraded determinate-nix\n"),
            1
        );
    }

    #[test]
    fn test_count_nix_store_ops_zero() {
        assert_eq!(count_nix_store_operations("nothing to do"), 0);
    }

    #[test]
    fn test_count_nix_store_ops_build_only() {
        let output = "these 3 derivations will be built:\n  /nix/store/foo.drv\n";
        assert_eq!(count_nix_store_operations(output), 3);
    }

    #[test]
    fn test_count_nix_store_ops_fetch_only() {
        let output = "these 5 paths will be fetched (10 MiB download, 50 MiB unpacked):\n";
        assert_eq!(count_nix_store_operations(output), 5);
    }

    #[test]
    fn test_count_nix_store_ops_build_and_fetch() {
        let output =
            "these 2 derivations will be built:\nthese 5 paths will be fetched (10 MiB download, 50 MiB unpacked):";
        assert_eq!(count_nix_store_operations(output), 7);
    }

    #[test]
    fn test_compare_lock_nodes_no_change() {
        let json = serde_json::json!({"nodes": {"nixpkgs": {"locked": {"rev": "abc", "lastModified": 100}}}});
        assert!(compare_lock_nodes(&json, &json).is_empty());
    }

    #[test]
    fn test_compare_lock_nodes_changed_rev() {
        let old = serde_json::json!({"nodes": {"nixpkgs": {"locked": {"rev": "abc", "lastModified": 100}}}});
        let new = serde_json::json!({"nodes": {"nixpkgs": {"locked": {"rev": "def", "lastModified": 100}}}});
        let changed = compare_lock_nodes(&old, &new);
        assert_eq!(changed, vec!["nixpkgs"]);
    }

    #[test]
    fn test_compare_lock_nodes_new_input_added() {
        let old = serde_json::json!({"nodes": {"nixpkgs": {"locked": {"rev": "abc", "lastModified": 100}}}});
        let new = serde_json::json!({"nodes": {
            "nixpkgs": {"locked": {"rev": "abc", "lastModified": 100}},
            "home-manager": {"locked": {"rev": "xyz", "lastModified": 200}}
        }});
        let changed = compare_lock_nodes(&old, &new);
        assert_eq!(changed, vec!["home-manager"]);
    }

    // validate_flake_attr tests (pure function, no system access)

    #[test]
    fn validate_flake_attr_accepts_valid_names() {
        assert!(validate_flake_attr("vexos-nvidia").is_ok());
        assert!(validate_flake_attr("my_host.01").is_ok());
        assert!(validate_flake_attr("a").is_ok());
    }

    #[test]
    fn validate_flake_attr_rejects_empty() {
        assert!(validate_flake_attr("").is_err());
    }

    #[test]
    fn validate_flake_attr_rejects_special_chars() {
        assert!(validate_flake_attr("host name").is_err());
        assert!(validate_flake_attr("host@domain").is_err());
        assert!(validate_flake_attr("host/path").is_err());
    }

    #[test]
    fn validate_flake_attr_rejects_too_long() {
        let long = "a".repeat(254);
        assert!(validate_flake_attr(&long).is_err());
    }

    // run_update pipeline tests — legacy nix-env branch.
    //
    // The NixOS flake, NixOS channel, and Determinate Nix run_update branches each begin
    // with OS-detection (is_nixos, is_nixos_flake, is_determinate_nix) that reads
    // /run/current-system, /etc/os-release, /nix/receipt.json etc., making them impossible
    // to exercise in unit tests without a SystemProber abstraction. The modern nix profile
    // branch calls nix_profile_upgrade_all() directly without going through runner, so it
    // is also not injectable via MockExecutor. Full run_update pipeline coverage for those
    // paths is deferred until a SystemProber trait is introduced.
    //
    // The legacy nix-env branch reads $HOME/.nix-profile/manifest.json, which we control
    // in tests by pointing HOME at a temporary directory.

    #[tokio::test]
    async fn run_update_legacy_nix_env_success() {
        if is_nixos() || is_determinate_nix() {
            return;
        }
        let tmp_home =
            std::env::temp_dir().join(format!("up-test-nix-{}-legacy-ok", std::process::id()));
        let nix_profile = tmp_home.join(".nix-profile");
        std::fs::create_dir_all(&nix_profile).unwrap();
        std::fs::write(
            nix_profile.join("manifest.json"),
            r#"{"version": 1, "elements": []}"#,
        )
        .unwrap();

        let prev_home = std::env::var("HOME").unwrap_or_default();
        let _home_guard = HOME_ENV_LOCK.lock().unwrap();
        std::env::set_var("HOME", &tmp_home);

        let executor = MockExecutor::with_output("upgrading 'hello-2.10' to 'hello-2.12'\n");
        let result = NixBackend.run_update(&executor).await;

        std::env::set_var("HOME", prev_home);
        drop(_home_guard);
        let _ = std::fs::remove_dir_all(&tmp_home);

        match result {
            UpdateResult::Success { updated_count } => assert_eq!(updated_count, 1),
            other => panic!("Expected Success {{ updated_count: 1 }}, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn run_update_legacy_nix_env_error() {
        if is_nixos() || is_determinate_nix() {
            return;
        }
        let tmp_home =
            std::env::temp_dir().join(format!("up-test-nix-{}-legacy-err", std::process::id()));
        let nix_profile = tmp_home.join(".nix-profile");
        std::fs::create_dir_all(&nix_profile).unwrap();
        std::fs::write(
            nix_profile.join("manifest.json"),
            r#"{"version": 1, "elements": []}"#,
        )
        .unwrap();

        let prev_home = std::env::var("HOME").unwrap_or_default();
        let _home_guard = HOME_ENV_LOCK.lock().unwrap();
        std::env::set_var("HOME", &tmp_home);

        let executor = MockExecutor::with_error(1, "nix-env: error upgrading packages");
        let result = NixBackend.run_update(&executor).await;

        std::env::set_var("HOME", prev_home);
        drop(_home_guard);
        let _ = std::fs::remove_dir_all(&tmp_home);

        assert!(
            matches!(result, UpdateResult::Error(_)),
            "Expected Error, got {:?}",
            result
        );
    }
}
