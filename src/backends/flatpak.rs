use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::runner::CommandRunner;
use std::future::Future;
use std::pin::Pin;

/// GitHub repository slug (owner/repo) used for self-update release checks.
const GITHUB_REPO: &str = "VictoryTek/Up";

/// Expected URL prefix for validated release asset downloads.
/// Any URL from the GitHub Releases API that does not start with this prefix
/// is rejected before it is embedded in a shell command.
const GITHUB_RELEASE_DOWNLOAD_PREFIX: &str = "https://github.com/VictoryTek/Up/releases/download/";

/// Temporary path on the host where the self-update bundle is downloaded
/// before installation.
const SELF_UPDATE_TMP_PATH: &str = "/tmp/up-self-update.flatpak";

/// Returns `true` when the current process is running inside a Flatpak sandbox.
///
/// Detection relies on the presence of `/.flatpak-info`, a metadata file that
/// Flatpak always creates inside the sandbox (documented in flatpak-metadata(5)).
/// This is more reliable than checking the `FLATPAK_ID` environment variable,
/// which could theoretically be spoofed.
pub fn is_running_in_flatpak() -> bool {
    std::path::Path::new("/.flatpak-info").exists()
}

/// Returns `true` when the Flatpak backend can operate on this system.
///
/// Inside a Flatpak sandbox `flatpak` itself is not on the sandbox PATH, but
/// `flatpak-spawn` (part of the GNOME Platform runtime) is available and can
/// execute host commands.  Outside the sandbox the plain `flatpak` binary is
/// required.
pub fn is_available() -> bool {
    if is_running_in_flatpak() {
        // Inside the sandbox `flatpak-spawn` routes commands to the host.
        which::which("flatpak-spawn").is_ok()
    } else {
        which::which("flatpak").is_ok()
    }
}

/// Returns `(program, args_vec)` for running a Flatpak subcommand.
///
/// When inside a Flatpak sandbox the command is prefixed with
/// `flatpak-spawn --host` so it executes on the host system with the host's
/// own network access and Flatpak installation — no sandbox network permission
/// is required.
fn build_flatpak_cmd(sub_args: &[&str]) -> (String, Vec<String>) {
    if is_running_in_flatpak() {
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

/// Parse a semver-like string (`"1.2.3"` or `"v1.2.3"`) into a
/// `(major, minor, patch)` tuple. Returns `None` if the string cannot be
/// parsed as three non-negative integers; the caller treats `None` as
/// "do not update" (safe default).
fn parse_semver(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches('v');
    let mut parts = s.splitn(3, '.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    // Split on the first non-digit so pre-release suffixes (e.g. "-beta.1")
    // do not prevent parsing entirely.
    let patch = parts
        .next()?
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse::<u32>()
        .ok()?;
    Some((major, minor, patch))
}

/// Returns `true` if `candidate_tag` (e.g., `"v1.2.0"`) is strictly newer
/// than the version compiled into this binary (`CARGO_PKG_VERSION`).
///
/// Returns `false` on any parse failure — safe default is to not self-update.
fn is_newer_than_current(candidate_tag: &str) -> bool {
    let current = parse_semver(env!("CARGO_PKG_VERSION"));
    let candidate = parse_semver(candidate_tag);
    match (current, candidate) {
        (Some(cur), Some(cand)) => cand > cur,
        _ => false,
    }
}

/// Query the GitHub Releases API for the latest release of Up and return
/// `(tag_name, download_url)`.
///
/// Inside the Flatpak sandbox the request is routed through
/// `flatpak-spawn --host` so the host network stack is used (no
/// `--share=network` permission required).  Outside the sandbox `python3`
/// is invoked directly — useful for local development and testing.
///
/// Returns `Err` if the command fails or the output cannot be parsed into
/// a non-empty tag line.
async fn fetch_github_latest_release(runner: &CommandRunner) -> Result<(String, String), String> {
    let output = if is_running_in_flatpak() {
        // Use curl + python3 on the host; the script prints exactly two lines:
        // the release tag and the first .flatpak asset URL.
        let script = format!(
            "curl -fsSL --connect-timeout 10 --max-time 30 --user-agent 'io.github.up/{ver}' \
             'https://api.github.com/repos/{repo}/releases/latest' \
             | python3 -c \
             \"import sys,json;\
r=json.load(sys.stdin);\
t=r.get('tag_name','');\
a=[x.get('browser_download_url','') for x in r.get('assets',[]) \
if x.get('name','').endswith('.flatpak')];\
print(t);print(a[0] if a else '')\"",
            ver = env!("CARGO_PKG_VERSION"),
            repo = GITHUB_REPO,
        );
        runner
            .run("flatpak-spawn", &["--host", "bash", "-c", &*script])
            .await
    } else {
        // Outside the sandbox: python3 can reach the network directly.
        let script = format!(
            "import urllib.request,json;\
r=urllib.request.urlopen(\
'https://api.github.com/repos/{repo}/releases/latest',timeout=10);\
d=json.loads(r.read());\
t=d.get('tag_name','');\
a=[x.get('browser_download_url','') for x in d.get('assets',[]) \
if x.get('name','').endswith('.flatpak')];\
print(t);print(a[0] if a else '')",
            repo = GITHUB_REPO,
        );
        runner.run("python3", &["-c", &*script]).await
    };

    let output = output.map_err(|e| format!("GitHub release check failed: {e}"))?;

    let mut lines = output.lines();
    let tag = lines.next().unwrap_or("").trim().to_string();
    let url = lines.next().unwrap_or("").trim().to_string();

    if tag.is_empty() {
        return Err("GitHub API returned no release tag".to_string());
    }

    Ok((tag, url))
}

/// Download the Flatpak bundle at `url` to a temporary path on the host and
/// reinstall it via `flatpak install --bundle --reinstall --user -y`.
///
/// **Security**: `url` must start with [`GITHUB_RELEASE_DOWNLOAD_PREFIX`].
/// Any other prefix is rejected before the URL is embedded in a shell command.
///
/// The running process is not terminated by the reinstall; the new version
/// takes effect on the next launch — exactly what the restart banner prompts.
async fn download_and_install_bundle(runner: &CommandRunner, url: &str) -> Result<(), String> {
    // Reject URLs that do not originate from the expected GitHub release path.
    if !url.starts_with(GITHUB_RELEASE_DOWNLOAD_PREFIX) {
        return Err(format!(
            "Rejected download URL with unexpected prefix: {url}"
        ));
    }

    // Reject URLs that contain a single-quote, which would break bash quoting.
    if url.contains('\'') {
        return Err(format!(
            "Rejected download URL containing invalid character: {url}"
        ));
    }

    // GitHub release URLs contain only HTTPS-safe characters and never include
    // single-quote characters, so single-quoting in bash is safe here.
    let script = format!(
        "curl -fsSL --connect-timeout 10 --max-time 300 -o '{tmp}' '{url}' && \
         flatpak install --bundle --reinstall --user -y '{tmp}'; \
         rm -f '{tmp}'",
        tmp = SELF_UPDATE_TMP_PATH,
        url = url,
    );

    runner
        .run("flatpak-spawn", &["--host", "bash", "-c", &*script])
        .await
        .map(|_| ())
        .map_err(|e| format!("Self-update install failed: {e}"))
}

pub struct FlatpakBackend;

impl Backend for FlatpakBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Flatpak
    }
    fn display_name(&self) -> &str {
        "Flatpak"
    }
    fn description(&self) -> &str {
        "Flatpak applications"
    }
    fn icon_name(&self) -> &str {
        "system-software-install-symbolic"
    }

    fn run_update<'a>(
        &'a self,
        runner: &'a CommandRunner,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            let (cmd, args) = build_flatpak_cmd(&["update", "-y"]);
            let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

            match runner.run(&cmd, &args_refs).await {
                Ok(output) => {
                    // Flatpak shows a table of updates; lines starting with a number
                    // indicate an actual update operation.
                    let count = output
                        .lines()
                        .filter(|l| {
                            let t = l.trim();
                            t.starts_with(|c: char| c.is_ascii_digit())
                        })
                        .count();

                    // When running inside the sandbox, detect whether Up itself was
                    // updated so the UI can prompt the user to restart.
                    let updated_self = is_running_in_flatpak()
                        && output.lines().any(|l| {
                            let t = l.trim();
                            t.starts_with(|c: char| c.is_ascii_digit()) && t.contains(crate::APP_ID)
                        });

                    // If running inside the Flatpak sandbox and Up was NOT
                    // updated via an OSTree remote, check GitHub Releases for
                    // a bundle update. Errors and "already up-to-date" cases
                    // are logged as warnings but do not prevent the normal
                    // success result from being returned.
                    let github_self_updated = if !updated_self && is_running_in_flatpak() {
                        match fetch_github_latest_release(runner).await {
                            Ok((tag, url)) if is_newer_than_current(&tag) && !url.is_empty() => {
                                match download_and_install_bundle(runner, &url).await {
                                    Ok(()) => {
                                        log::info!(
                                            "Self-update from GitHub Releases: installed {}",
                                            tag
                                        );
                                        true
                                    }
                                    Err(e) => {
                                        log::warn!("Self-update install error: {e}");
                                        false
                                    }
                                }
                            }
                            Ok((tag, _)) => {
                                log::info!("Self-update check: already at latest ({tag})");
                                false
                            }
                            Err(e) => {
                                log::warn!("Self-update check error: {e}");
                                false
                            }
                        }
                    } else {
                        false
                    };

                    if updated_self || github_self_updated {
                        UpdateResult::SuccessWithSelfUpdate {
                            updated_count: count,
                        }
                    } else {
                        UpdateResult::Success {
                            updated_count: count,
                        }
                    }
                }
                Err(e) => UpdateResult::Error(e),
            }
        })
    }

    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async move {
            // Use --dry-run -y so the resolution logic matches run_update() exactly,
            // including runtimes and extensions. The -y flag forces non-interactive
            // mode so Flatpak always prints its numbered update table even when
            // stdout is a pipe (no TTY) — without it the table is suppressed and
            // the count always returns 0. Format stable since Flatpak 1.2.0.
            let (cmd, args) = build_flatpak_cmd(&["update", "--dry-run", "-y"]);
            let out = tokio::process::Command::new(&cmd)
                .args(&args)
                .output()
                .await
                .map_err(|e| e.to_string())?;
            // Combine stdout and stderr: some Flatpak versions write the table
            // to stderr, so reading only stdout would miss all updates.
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let combined = format!("{stdout}{stderr}");
            Ok(combined
                .lines()
                .filter(|l| {
                    let t = l.trim();
                    t.starts_with(|c: char| c.is_ascii_digit())
                })
                .count())
        })
    }

    fn list_available(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
        Box::pin(async move {
            let (cmd, args) = build_flatpak_cmd(&["update", "--dry-run", "-y"]);
            let out = tokio::process::Command::new(&cmd)
                .args(&args)
                .output()
                .await
                .map_err(|e| e.to_string())?;
            // Combine stdout and stderr: some Flatpak versions write the table
            // to stderr, so reading only stdout would miss all updates.
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let combined = format!("{stdout}{stderr}");
            // Lines are either the modern format (no brackets):
            //   " 1.     com.example.App  stable  u  flathub  50.1 MB"
            // or the legacy bracket format (Flatpak < 1.6):
            //   " 1. [✓] com.example.App  stable  u  flathub  50.1 MB"
            Ok(combined
                .lines()
                .filter(|l| {
                    let t = l.trim();
                    t.starts_with(|c: char| c.is_ascii_digit())
                })
                .filter_map(|l| {
                    let t = l.trim();
                    // Strip the leading "N." number prefix (handles 1–N digit numbers).
                    let rest = t
                        .trim_start_matches(|c: char| c.is_ascii_digit())
                        .trim_start_matches(['.', '\t', ' ']);
                    // Skip optional "[✓]" / "[i]" bracket marker (legacy Flatpak).
                    let name_part = if rest.starts_with('[') {
                        rest.splitn(2, ']').nth(1).unwrap_or("").trim()
                    } else {
                        rest
                    };
                    let name = name_part.split_whitespace().next()?;
                    if name.is_empty() {
                        None
                    } else {
                        Some(name.to_string())
                    }
                })
                .collect())
        })
    }
}
