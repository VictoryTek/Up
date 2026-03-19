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

    fn run_update<'a>(&'a self, runner: &'a CommandRunner) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
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
            let out = tokio::process::Command::new("flatpak")
                .args(["remote-ls", "--updates"])
                .output()
                .await
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            Ok(text.lines().filter(|l| !l.is_empty()).count())
        })
    }
}
