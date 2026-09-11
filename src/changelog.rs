use crate::backends::BackendKind;
use tokio::time::{timeout, Duration};

/// Whether [`fetch_changelog`] can return anything for this backend.
///
/// Nix is supported only on flake-based NixOS, where the flake inputs give a
/// meaningful revision delta. Plugin backends are never supported.
pub fn supports_changelog(kind: &BackendKind) -> bool {
    match kind {
        BackendKind::Nix => crate::backends::nix::supports_lock_changelog(),
        BackendKind::Plugin(_) => false,
        _ => true,
    }
}

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
/// changelog fetching (plugins, and Nix outside flake-based NixOS).
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
        BackendKind::Nix => fetch_nix().await,
        BackendKind::Plugin(_) => Err(ChangelogError::NotSupported),
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

/// Maximum pending Flatpak refs to fetch commit logs for in one dialog.
const MAX_FLATPAK_ENTRIES: usize = 5;

/// Flatpak: resolve pending updates to exact refs plus their originating remote,
/// then fetch the commit log for each (capped at [`MAX_FLATPAK_ENTRIES`]).
///
/// Deliberately re-enumerates the pending refs rather than reusing the caller's
/// `packages` list: that list comes from `remote-ls --columns=name`, which
/// resolves to the appstream *display* name ("Firefox") whenever appstream data
/// is cached, and `remote-info` cannot resolve a display name. Asking for
/// `--columns=ref,origin` gives unambiguous identifiers for apps and runtimes
/// alike, and the exact remote instead of a `flathub` guess.
///
/// Respects the Flatpak sandbox by prefixing commands with `flatpak-spawn --host`
/// when running inside one.
async fn fetch_flatpak(packages: &[String]) -> Result<String, ChangelogError> {
    if packages.is_empty() {
        return Ok("No updates pending.".to_string());
    }

    // (scope, remote, ref) for every pending update, across both installations.
    let mut targets: Vec<(String, String, String)> = Vec::new();
    let mut list_errors: Vec<String> = Vec::new();
    for scope in ["--user", "--system"] {
        let (prog, args) =
            build_flatpak_cmd(&["remote-ls", "--updates", scope, "--columns=ref,origin"]);
        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        match run_cmd(&prog, &args_refs).await {
            Ok(out) => {
                for target in parse_flatpak_ref_origin(&out, scope) {
                    if !targets.iter().any(|(_, _, r)| *r == target.2) {
                        targets.push(target);
                    }
                }
            }
            Err(e) => list_errors.push(e.to_string()),
        }
    }

    if targets.is_empty() {
        // Both scopes failing is a real error, not an absence of changelog data.
        return match list_errors.into_iter().next() {
            Some(e) => Err(ChangelogError::Exit(-1, e)),
            None => Ok("No changelog information available.".to_string()),
        };
    }

    let mut results = Vec::new();
    for (scope, remote, reference) in targets.iter().take(MAX_FLATPAK_ENTRIES) {
        let (prog, args) = build_flatpak_cmd(&["remote-info", scope, "--log", remote, reference]);
        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        match run_cmd(&prog, &args_refs).await {
            Ok(text) => results.push(text.trim().to_string()),
            // Report per-ref failures rather than silently dropping them.
            Err(e) => results.push(format!("{reference}\n  Changelog unavailable: {e}")),
        }
    }

    Ok(truncate(results.join("\n---\n")))
}

/// Parse `flatpak remote-ls --updates <scope> --columns=ref,origin` output into
/// `(scope, remote, ref)` triples.
///
/// The column header is rejected by requiring a `/` in the first field: every
/// real ref is `app/<id>/<arch>/<branch>` or `runtime/<id>/<arch>/<branch>`,
/// and no header label contains one.
fn parse_flatpak_ref_origin(output: &str, scope: &str) -> Vec<(String, String, String)> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let reference = parts.next()?;
            let origin = parts.next()?;
            if !reference.contains('/') {
                return None;
            }
            Some((scope.to_string(), origin.to_string(), reference.to_string()))
        })
        .collect()
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

/// Maximum flake inputs to describe in one dialog.
const MAX_NIX_INPUTS: usize = 10;

/// Budget for `nix flake update`, which fetches every input over the network.
/// Far longer than [`run_cmd`]'s 30s: this runs on a background thread in
/// response to an explicit user action, behind a progress placeholder.
const NIX_TIMEOUT: Duration = Duration::from_secs(180);

/// Nix: report which flake inputs moved and how far.
///
/// Nix has no per-package changelog, but on a flake-based NixOS the inputs are
/// the unit that actually changes, so the pending revision delta for each input
/// is the meaningful answer to "what's new".
async fn fetch_nix() -> Result<String, ChangelogError> {
    if !crate::backends::nix::supports_lock_changelog() {
        return Err(ChangelogError::NotSupported);
    }

    let deltas = timeout(NIX_TIMEOUT, crate::backends::nix::nixos_flake_lock_deltas())
        .await
        .map_err(|_| ChangelogError::Spawn("nix flake update: timed out after 180s".to_string()))?
        .map_err(|e| ChangelogError::Exit(-1, e))?;

    if deltas.is_empty() {
        return Ok("All flake inputs are up to date.".to_string());
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut out = String::new();
    for delta in deltas.iter().take(MAX_NIX_INPUTS) {
        out.push_str(&delta.name);
        out.push('\n');
        match &delta.old_rev {
            Some(rev) => out.push_str(&format!(
                "  Current: {}{}\n",
                short_rev(rev),
                age_suffix(delta.old_modified, now)
            )),
            None => out.push_str("  Current: (new input)\n"),
        }
        if let Some(rev) = &delta.new_rev {
            out.push_str(&format!(
                "  New:     {}{}\n",
                short_rev(rev),
                age_suffix(delta.new_modified, now)
            ));
        }
        if let (Some((owner, repo)), Some(old), Some(new)) =
            (&delta.github, &delta.old_rev, &delta.new_rev)
        {
            out.push_str(&format!(
                "  Compare: https://github.com/{owner}/{repo}/compare/{}...{}\n",
                short_rev(old),
                short_rev(new)
            ));
        }
        out.push('\n');
    }

    if deltas.len() > MAX_NIX_INPUTS {
        out.push_str(&format!(
            "[...and {} more input(s)]\n",
            deltas.len() - MAX_NIX_INPUTS
        ));
    }
    Ok(truncate(out))
}

/// Abbreviate a git revision to the conventional 8-character short form.
fn short_rev(rev: &str) -> &str {
    &rev[..rev.len().min(8)]
}

/// Render a `lastModified` timestamp as a parenthesised relative age, e.g.
/// `" (14 days ago)"`. Empty when the timestamp is absent or in the future.
///
/// Deliberately integer arithmetic on whole days rather than a calendar
/// dependency: relative age is what matters here, not an exact date.
fn age_suffix(modified: Option<i64>, now: i64) -> String {
    let Some(modified) = modified else {
        return String::new();
    };
    let days = (now - modified) / 86_400;
    match days {
        d if d < 0 => String::new(),
        0 => " (today)".to_string(),
        1 => " (1 day ago)".to_string(),
        d => format!(" ({d} days ago)"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ref_origin_skips_header_and_keeps_apps_and_runtimes() {
        let output = "Ref\tOrigin\n\
                      app/org.gnome.Calculator/x86_64/stable\tflathub\n\
                      runtime/org.gnome.Platform/x86_64/47\tflathub\n";
        assert_eq!(
            parse_flatpak_ref_origin(output, "--user"),
            vec![
                (
                    "--user".to_string(),
                    "flathub".to_string(),
                    "app/org.gnome.Calculator/x86_64/stable".to_string()
                ),
                (
                    "--user".to_string(),
                    "flathub".to_string(),
                    "runtime/org.gnome.Platform/x86_64/47".to_string()
                ),
            ]
        );
    }

    #[test]
    fn parse_ref_origin_handles_multiple_remotes() {
        let output = "app/com.example.App/x86_64/stable\tfedora\n\
                      app/org.gnome.Maps/x86_64/stable\tflathub\n";
        let parsed = parse_flatpak_ref_origin(output, "--system");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].1, "fedora");
        assert_eq!(parsed[1].1, "flathub");
    }

    #[test]
    fn parse_ref_origin_empty_and_header_only() {
        assert!(parse_flatpak_ref_origin("", "--user").is_empty());
        assert!(parse_flatpak_ref_origin("Ref\tOrigin\n", "--user").is_empty());
        // A ref with no origin column is unusable and must be dropped.
        assert!(
            parse_flatpak_ref_origin("app/org.gnome.Maps/x86_64/stable\n", "--user").is_empty()
        );
    }

    #[test]
    fn short_rev_abbreviates_and_tolerates_short_input() {
        assert_eq!(short_rev("a1b2c3d4e5f6a7b8"), "a1b2c3d4");
        assert_eq!(short_rev("abc"), "abc");
        assert_eq!(short_rev(""), "");
    }

    #[test]
    fn age_suffix_renders_relative_days() {
        let now = 1_000_000_000_i64;
        assert_eq!(age_suffix(Some(now), now), " (today)");
        assert_eq!(age_suffix(Some(now - 86_400), now), " (1 day ago)");
        assert_eq!(age_suffix(Some(now - 14 * 86_400), now), " (14 days ago)");
        assert_eq!(age_suffix(None, now), "");
        // A clock skew putting the input in the future must not render nonsense.
        assert_eq!(age_suffix(Some(now + 5 * 86_400), now), "");
    }
}
