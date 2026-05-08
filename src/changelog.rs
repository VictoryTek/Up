use crate::backends::BackendKind;
use tokio::time::{timeout, Duration};

#[derive(Debug, thiserror::Error)]
pub enum ChangelogError {
    #[error("Changelog not available for this backend")]
    NotSupported,
    #[error("Command failed (exit {0}): {1}")]
    Exit(i32, String),
    #[error("Failed to run command: {0}")]
    Spawn(String),
}

const MAX_CHARS: usize = 10_000;

fn truncate(s: String) -> String {
    if s.len() > MAX_CHARS {
        format!("{}\n[...truncated]", &s[..MAX_CHARS])
    } else {
        s
    }
}

/// Fetch changelog / release-notes text for `packages` (pending update names)
/// from the given backend.
///
/// Returns `Err(ChangelogError::NotSupported)` for backends that do not support
/// changelog fetching (Nix only at present).
///
/// All commands are unprivileged read-only queries. Call from a background thread.
pub async fn fetch_changelog(
    kind: BackendKind,
    packages: &[String],
) -> Result<String, ChangelogError> {
    match kind {
        BackendKind::Apt => fetch_apt(packages).await,
        BackendKind::Dnf => fetch_dnf(packages).await,
        BackendKind::Pacman => fetch_pacman(packages).await,
        BackendKind::Zypper => fetch_zypper(packages).await,
        BackendKind::Flatpak => fetch_flatpak(packages).await,
        BackendKind::Homebrew => fetch_homebrew(packages).await,
        BackendKind::Fwupd => fetch_fwupd().await,
        BackendKind::Nix => Err(ChangelogError::NotSupported),
    }
}

/// Run a command with a 30-second timeout and `LANG=C`, returning stdout on
/// success or a `ChangelogError` on failure.
async fn run_cmd(program: &str, args: &[&str]) -> Result<String, ChangelogError> {
    let out = timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(program)
            .args(args)
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .output(),
    )
    .await
    .map_err(|_| ChangelogError::Spawn(format!("{program}: timed out after 30s")))?
    .map_err(|e| ChangelogError::Spawn(e.to_string()))?;

    if !out.status.success() {
        let code = out.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        return Err(ChangelogError::Exit(code, stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// APT: fetch package metadata for up to 20 pending packages using
/// `apt-cache show --no-all-versions` (offline, reads local package cache).
async fn fetch_apt(packages: &[String]) -> Result<String, ChangelogError> {
    if packages.is_empty() {
        return Ok("No packages pending.".to_string());
    }
    let pkgs: Vec<&str> = packages.iter().take(20).map(String::as_str).collect();
    let mut args: Vec<&str> = vec!["show", "--no-all-versions"];
    args.extend(pkgs.iter().copied());
    let output = run_cmd("apt-cache", &args).await?;
    Ok(truncate(output))
}

/// DNF: run `dnf updateinfo info --updates` (no per-package args needed).
async fn fetch_dnf(_packages: &[String]) -> Result<String, ChangelogError> {
    let output = run_cmd("dnf", &["updateinfo", "info", "--updates"]).await?;
    Ok(truncate(output))
}

/// Pacman: show metadata for up to 10 pending packages via `pacman -Si`.
async fn fetch_pacman(packages: &[String]) -> Result<String, ChangelogError> {
    if packages.is_empty() {
        return Ok("No packages pending.".to_string());
    }
    let pkgs: Vec<&str> = packages.iter().take(10).map(String::as_str).collect();
    let mut args: Vec<&str> = vec!["-Si"];
    args.extend(pkgs.iter().copied());
    let output = run_cmd("pacman", &args).await?;
    Ok(truncate(output))
}

/// Zypper: show package information for up to 10 pending packages.
async fn fetch_zypper(packages: &[String]) -> Result<String, ChangelogError> {
    if packages.is_empty() {
        return Ok("No packages pending.".to_string());
    }
    let pkgs: Vec<&str> = packages.iter().take(10).map(String::as_str).collect();
    let mut args: Vec<&str> = vec!["info"];
    args.extend(pkgs.iter().copied());
    let output = run_cmd("zypper", &args).await?;
    Ok(truncate(output))
}

/// Flatpak: two-step — get app→remote mappings, then fetch the commit log for
/// each pending app (capped at 5). Respects the Flatpak sandbox by prefixing
/// commands with `flatpak-spawn --host` when running inside a sandbox.
async fn fetch_flatpak(packages: &[String]) -> Result<String, ChangelogError> {
    if packages.is_empty() {
        return Ok("No updates pending.".to_string());
    }

    let (list_prog, list_args) =
        build_flatpak_cmd(&["list", "--app", "--columns=application,origin"]);
    let list_refs: Vec<&str> = list_args.iter().map(|s| s.as_str()).collect();
    let list_output = run_cmd(&list_prog, &list_refs).await?;

    let remote_map: std::collections::HashMap<String, String> = list_output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let app = parts.next()?.to_string();
            let remote = parts.next()?.to_string();
            Some((app, remote))
        })
        .collect();

    let apps_to_show = packages.len().min(5);
    let mut results = Vec::new();
    for app_id in &packages[..apps_to_show] {
        let remote = remote_map
            .get(app_id.as_str())
            .map(|s| s.as_str())
            .unwrap_or("flathub");
        let (prog, args) = build_flatpak_cmd(&["remote-info", "--log", remote, app_id.as_str()]);
        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        if let Ok(text) = run_cmd(&prog, &args_refs).await {
            results.push(text);
        }
    }

    if results.is_empty() {
        return Ok("No changelog information available.".to_string());
    }
    Ok(truncate(results.join("\n---\n")))
}

fn build_flatpak_cmd(sub_args: &[&str]) -> (String, Vec<String>) {
    if crate::backends::flatpak::is_running_in_flatpak() {
        let mut args = vec!["--host".to_string(), "flatpak".to_string()];
        args.extend(sub_args.iter().map(|s| s.to_string()));
        ("flatpak-spawn".to_string(), args)
    } else {
        (
            "flatpak".to_string(),
            sub_args.iter().map(|s| s.to_string()).collect(),
        )
    }
}

/// Homebrew: show formula information for up to 5 pending packages.
async fn fetch_homebrew(packages: &[String]) -> Result<String, ChangelogError> {
    if packages.is_empty() {
        return Ok("No packages pending.".to_string());
    }
    let pkgs: Vec<&str> = packages.iter().take(5).map(String::as_str).collect();
    let mut args: Vec<&str> = vec!["info"];
    args.extend(pkgs.iter().copied());
    let output = run_cmd("brew", &args).await?;
    Ok(truncate(output))
}

/// fwupd: run `fwupdmgr get-updates --json`, extract device names and release
/// descriptions. Falls back to raw stdout if JSON parsing fails.
/// Exit code 2 means "no updates available" and is treated as a success.
async fn fetch_fwupd() -> Result<String, ChangelogError> {
    let out = timeout(
        Duration::from_secs(30),
        tokio::process::Command::new("fwupdmgr")
            .args(["get-updates", "--json"])
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .output(),
    )
    .await
    .map_err(|_| ChangelogError::Spawn("fwupdmgr: timed out after 30s".to_string()))?
    .map_err(|e| ChangelogError::Spawn(e.to_string()))?;

    // Exit code 2 = no updates available — documented success state for fwupdmgr.
    if out.status.code() == Some(2) {
        return Ok("No firmware updates available.".to_string());
    }
    if !out.status.success() {
        let code = out.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        return Err(ChangelogError::Exit(code, stderr));
    }

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();

    // Try to parse JSON and extract per-device release descriptions.
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&stdout) {
        if let Some(devices) = val.get("Devices").and_then(|d| d.as_array()) {
            let mut output = String::new();
            for device in devices {
                let name = device
                    .get("Name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("Unknown Device");
                output.push_str(&format!("Device: {name}\n"));
                if let Some(releases) = device.get("Releases").and_then(|r| r.as_array()) {
                    for release in releases {
                        let version = release
                            .get("Version")
                            .and_then(|v| v.as_str())
                            .unwrap_or("?");
                        let desc = release
                            .get("Description")
                            .and_then(|d| d.as_str())
                            .unwrap_or("")
                            .trim();
                        if !desc.is_empty() {
                            output.push_str(&format!("  v{version}: {desc}\n"));
                        } else {
                            output.push_str(&format!("  v{version}\n"));
                        }
                    }
                }
                output.push('\n');
            }
            if !output.is_empty() {
                return Ok(truncate(output));
            }
        }
    }

    // Fall back to raw stdout if JSON parsing fails or produces no output.
    Ok(truncate(stdout))
}
