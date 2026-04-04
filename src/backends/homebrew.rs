use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::runner::CommandRunner;
use std::future::Future;
use std::pin::Pin;

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

    fn run_update<'a>(
        &'a self,
        runner: &'a CommandRunner,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
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
        })
    }

    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("brew")
                .args(["outdated"])
                .output()
                .await
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            Ok(text.lines().filter(|l| !l.is_empty()).count())
        })
    }

    fn list_available(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("brew")
                .args(["outdated"])
                .output()
                .await
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            // Each line is "pkgname (old-version) < new-version" or just "pkgname"
            Ok(text
                .lines()
                .filter(|l| !l.is_empty())
                .filter_map(|l| l.split_whitespace().next().map(|s| s.to_string()))
                .collect())
        })
    }
}
