use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::runner::CommandRunner;
use std::future::Future;
use std::pin::Pin;

pub fn is_available() -> bool {
    which::which("flatpak").is_ok()
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
            match runner.run("flatpak", &["update", "-y"]).await {
                Ok(output) => {
                    // Count lines that mention "updating" or actual update ops
                    let count = output
                        .lines()
                        .filter(|l| {
                            let t = l.trim();
                            // Flatpak shows a table of updates; lines starting with a number
                            // indicate an update operation
                            t.starts_with(|c: char| c.is_ascii_digit())
                        })
                        .count();
                    UpdateResult::Success {
                        updated_count: count,
                    }
                }
                Err(e) => UpdateResult::Error(e),
            }
        })
    }

    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async move {
            // Use --dry-run so the resolution logic matches run_update() exactly,
            // including runtimes and extensions. Format stable since Flatpak 1.2.0.
            let out = tokio::process::Command::new("flatpak")
                .args(["update", "--dry-run"])
                .output()
                .await
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            Ok(text
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
            let out = tokio::process::Command::new("flatpak")
                .args(["update", "--dry-run"])
                .output()
                .await
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            // Lines format: " 1. [✓] com.app.Name  stable  u  flathub  1.0 MB"
            // Extract app ID from between ']' and first whitespace.
            Ok(text
                .lines()
                .filter(|l| {
                    let t = l.trim();
                    t.starts_with(|c: char| c.is_ascii_digit())
                })
                .filter_map(|l| {
                    l.trim()
                        .split(']')
                        .nth(1)
                        .unwrap_or("")
                        .trim()
                        .split_whitespace()
                        .next()
                        .map(|s| s.to_string())
                })
                .filter(|s| !s.is_empty())
                .collect())
        })
    }
}
