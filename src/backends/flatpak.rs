use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::runner::CommandRunner;

pub fn is_available() -> bool {
    which::which("flatpak").is_ok()
}

pub struct FlatpakBackend;

#[async_trait::async_trait]
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

    async fn run_update(&self, runner: &CommandRunner) -> UpdateResult {
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
    }
}
