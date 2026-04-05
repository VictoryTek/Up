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

/// Parse the combined stdout+stderr output of Nix build commands to count how
/// many store paths were actually built or fetched.
///
/// Nix emits lines of the form:
///   "these N derivations will be built:"
///   "these N paths will be fetched (X MiB download, Y MiB unpacked):"
///
/// These lines are only present when real work is done. When the system is
/// already up to date they are absent, so this function correctly returns 0.
fn count_nix_store_operations(output: &str) -> usize {
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
fn compare_lock_nodes(old: &serde_json::Value, new: &serde_json::Value) -> Vec<String> {
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

    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("Failed to create temp dir: {e}"))?;

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

    let temp_dir_str = temp_dir
        .to_str()
        .ok_or_else(|| cleanup("Temp dir path contains non-UTF-8 bytes".to_string()))?
        .to_string();

    // Run `nix flake update <tempdir>` — writes updated flake.lock in-place.
    let out = tokio::process::Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "flake",
            "update",
            &temp_dir_str,
        ])
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
                    // Single pkexec invocation so polkit only prompts once.
                    // pkexec resets PATH, so we restore the NixOS binary paths
                    // explicitly via `env PATH=...` before invoking sh.
                    // config_name is validated by validate_flake_attr (ASCII
                    // alphanumeric / hyphen / underscore / dot only), so it is
                    // safe to interpolate into the shell command string.
                    let cmd = format!(
                        "nix --extra-experimental-features 'nix-command flakes' \
                         flake update --flake /etc/nixos && \
                         nixos-rebuild switch --flake /etc/nixos#{}",
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
                                "nix-channel --update && nixos-rebuild switch",
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
                            updated_count: count_nix_store_operations(&output),
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
            if is_nixos() && is_nixos_flake() {
                // Flake-based NixOS: detect changed inputs via --dry-run or temp-dir.
                nixos_flake_changed_inputs().await.map(|v| v.len())
            } else {
                // Non-NixOS Nix profile or legacy NixOS channels: check user
                // profile upgrades via nix-env dry-run.
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

    fn list_available(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
        Box::pin(async move {
            if is_nixos() && is_nixos_flake() {
                // Flake-based NixOS: return changed input names.
                nixos_flake_changed_inputs().await
            } else {
                // Non-NixOS Nix profile or legacy NixOS channels.
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
            }
        })
    }
}
