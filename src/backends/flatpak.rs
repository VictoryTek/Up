use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::executor::CommandExecutor;
use std::future::Future;
use std::pin::Pin;

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
        runner: &'a dyn CommandExecutor,
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

                    // SECURITY: GitHub-direct self-update has been removed. Downloading and
                    // installing a Flatpak bundle without GPG/checksum verification is not
                    // acceptable. When Up is distributed via Flathub, `flatpak update -y` above
                    // handles self-updates via OSTree with full signature verification. A
                    // GitHub-direct path should only be re-added with minisign or GPG verification
                    // of the downloaded bundle against a key pinned in the source code.
                    let github_self_updated = false;

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

    fn list_available(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
        Box::pin(async move {
            // Use `flatpak remote-ls --updates --user --columns=application` to detect
            // pending updates without applying them. The `--columns=application` flag
            // ensures one application ID per line for predictable parsing.
            // The `--user` flag is intentional: the `--system` variant triggers a polkit
            // prompt on every background check, which is poor UX. System Flatpak installs
            // are uncommon on desktop systems, so only user installations are checked here.
            let (cmd, args) =
                build_flatpak_cmd(&["remote-ls", "--updates", "--user", "--columns=application"]);
            let out = tokio::process::Command::new(&cmd)
                .args(&args)
                .output()
                .await
                .map_err(|e| e.to_string())?;

            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(format!("flatpak remote-ls --updates failed: {stderr}"));
            }

            let text = String::from_utf8_lossy(&out.stdout);
            Ok(parse_flatpak_updates(&text))
        })
    }

    fn estimate_size(&self) -> Pin<Box<dyn Future<Output = Option<u64>> + Send + '_>> {
        Box::pin(async move {
            let (cmd, args) = build_flatpak_cmd(&[
                "remote-ls",
                "--updates",
                "--user",
                "--columns=download-size",
            ]);
            let out = tokio::process::Command::new(&cmd)
                .args(&args)
                .env("LANG", "C")
                .env("LC_ALL", "C")
                .output()
                .await
                .ok()?;
            if !out.status.success() {
                return None;
            }
            let text = String::from_utf8_lossy(&out.stdout);
            let total = crate::disk::parse_flatpak_sizes(&text);
            if total == 0 {
                None
            } else {
                Some(total)
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
            let (cmd, args) = build_flatpak_cmd(&["uninstall", "--unused", "-y"]);
            let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

            match runner.run(&cmd, &args_refs).await {
                Ok(output) => {
                    let removed = output
                        .lines()
                        .filter(|l| l.trim().starts_with("Uninstalling:"))
                        .count();
                    UpdateResult::Success {
                        updated_count: removed,
                    }
                }
                Err(e) => UpdateResult::Error(e),
            }
        })
    }
}

/// Parse full output from `flatpak remote-ls --updates --columns=application`,
/// returning a deduplicated list of application IDs.
pub(crate) fn parse_flatpak_updates(output: &str) -> Vec<String> {
    let mut apps: Vec<String> = Vec::new();
    for line in output.lines() {
        if let Some(app_id) = parse_flatpak_app_line(line) {
            if !apps.contains(&app_id) {
                apps.push(app_id);
            }
        }
    }
    apps
}

/// Parse a line from `flatpak update --no-deploy --columns=application` output.
/// Returns `Some(app_id)` for valid (non-empty, non-header) lines.
pub(crate) fn parse_flatpak_app_line(line: &str) -> Option<String> {
    let t = line.trim();
    if !t.is_empty() && !t.eq_ignore_ascii_case("application") {
        Some(t.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::BackendError;
    use crate::executor::test_utils::MockExecutor;

    #[test]
    fn test_parse_flatpak_app_line_valid() {
        assert_eq!(
            parse_flatpak_app_line("org.gnome.Calculator"),
            Some("org.gnome.Calculator".to_string())
        );
    }

    #[test]
    fn test_parse_flatpak_app_line_header_skipped() {
        assert_eq!(parse_flatpak_app_line("Application"), None);
        assert_eq!(parse_flatpak_app_line("application"), None);
    }

    #[test]
    fn test_parse_flatpak_app_line_empty_skipped() {
        assert_eq!(parse_flatpak_app_line(""), None);
        assert_eq!(parse_flatpak_app_line("   "), None);
    }

    #[test]
    fn test_parse_flatpak_app_line_trims_whitespace() {
        assert_eq!(
            parse_flatpak_app_line("  com.example.App  "),
            Some("com.example.App".to_string())
        );
    }

    #[test]
    fn test_parse_flatpak_updates_happy_path() {
        let output = "Application\norg.gnome.Calculator\ncom.example.App\n";
        let result = parse_flatpak_updates(output);
        assert_eq!(
            result,
            vec![
                "org.gnome.Calculator".to_string(),
                "com.example.App".to_string()
            ]
        );
    }

    #[test]
    fn test_parse_flatpak_updates_only_header() {
        assert!(parse_flatpak_updates("Application\n").is_empty());
    }

    #[test]
    fn test_parse_flatpak_updates_deduplicates() {
        let output = "Application\norg.gnome.Calculator\norg.gnome.Calculator\n";
        let result = parse_flatpak_updates(output);
        assert_eq!(result, vec!["org.gnome.Calculator".to_string()]);
    }

    // --- run_update pipeline tests ---

    #[tokio::test]
    async fn test_flatpak_run_update_with_updates() {
        let output = "Looking for updates...\n\n   ID                            Branch  Op  Remote  Download\n1. org.gnome.Calculator          stable  u   flathub 1.5 MB\n2. com.spotify.Client            stable  u   flathub 87.3 MB\n\nUpdating: org.gnome.Calculator/x86_64/stable from flathub\n";
        let mock = MockExecutor::with_output(output);
        let result = FlatpakBackend.run_update(&mock).await;
        assert!(
            matches!(result, UpdateResult::Success { updated_count: 2 }),
            "Expected Success {{ updated_count: 2 }}, got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_flatpak_run_update_nothing_to_do() {
        let mock = MockExecutor::with_output("Looking for updates...\n\nNothing to do.\n");
        let result = FlatpakBackend.run_update(&mock).await;
        assert!(
            matches!(result, UpdateResult::Success { updated_count: 0 }),
            "Expected Success {{ updated_count: 0 }}, got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_flatpak_run_update_error() {
        let mock = MockExecutor::new(vec![Err(BackendError::Exit {
            code: 1,
            message: "flatpak: error".into(),
        })]);
        let result = FlatpakBackend.run_update(&mock).await;
        assert!(matches!(result, UpdateResult::Error(_)));
    }
}
