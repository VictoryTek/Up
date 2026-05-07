use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::runner::CommandRunner;
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

    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async move { self.list_available().await.map(|v| v.len()) })
    }

    fn list_available(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
        Box::pin(async move {
            // Use `flatpak update --no-deploy -y --user --columns=application` to detect
            // pending updates without applying them. The `--columns=application` flag
            // ensures one application ID per line for predictable parsing.
            // The `--user` flag is intentional: the `--system` variant triggers a polkit
            // prompt on every background check, which is poor UX. System Flatpak installs
            // are uncommon on desktop systems, so only user installations are checked here.
            let (cmd, args) = build_flatpak_cmd(&[
                "update",
                "--no-deploy",
                "-y",
                "--user",
                "--columns=application",
            ]);
            let out = tokio::process::Command::new(&cmd)
                .args(&args)
                .output()
                .await
                .map_err(|e| e.to_string())?;

            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(format!("flatpak update --no-deploy failed: {stderr}"));
            }

            let text = String::from_utf8_lossy(&out.stdout);
            let mut apps: Vec<String> = Vec::new();
            for line in text.lines() {
                let t = line.trim();
                // With --columns=application, each line is one application ID.
                // Skip empty lines and the header line ("Application").
                if !t.is_empty() && !t.eq_ignore_ascii_case("application") {
                    let app_id = t.to_string();
                    if !apps.contains(&app_id) {
                        apps.push(app_id);
                    }
                }
            }
            Ok(apps)
        })
    }
}
