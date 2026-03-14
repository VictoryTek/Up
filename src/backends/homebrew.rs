use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::runner::CommandRunner;

pub fn is_available() -> bool {
    which::which("brew").is_ok()
}

pub struct HomebrewBackend;

impl Backend for HomebrewBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Homebrew
    }
    fn display_name(&self) -> &str {
        "Homebrew"
    }
    fn description(&self) -> &str {
        "Homebrew (Linuxbrew) packages"
    }
    fn icon_name(&self) -> &str {
        "system-software-install-symbolic"
    }

    async fn run_update(&self, runner: &CommandRunner) -> UpdateResult {
        if let Err(e) = runner.run("brew", &["update"]).await {
            return UpdateResult::Error(e);
        }
        match runner.run("brew", &["upgrade"]).await {
            Ok(output) => {
                let count = output
                    .lines()
                    .filter(|l| l.contains("Upgrading") || l.contains("Pouring"))
                    .count();
                UpdateResult::Success {
                    updated_count: count,
                }
            }
            Err(e) => UpdateResult::Error(e),
        }
    }
}
