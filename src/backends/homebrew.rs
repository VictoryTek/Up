use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::runner::CommandRunner;

pub fn is_available() -> bool {
    which::which("brew").is_ok()
}

pub struct HomebrewBackend;

#[async_trait::async_trait]
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

    async fn count_available(&self) -> Result<usize, String> {
        let out = tokio::process::Command::new("brew")
            .args(["outdated"])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(text.lines().filter(|l| !l.is_empty()).count())
    }
}
