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
                    let count = count_homebrew_upgraded(&output);
                    UpdateResult::Success {
                        updated_count: count,
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
            let out = tokio::process::Command::new("brew")
                .args(["outdated"])
                .output()
                .await
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            Ok(parse_brew_outdated(&text))
        })
    }
}

pub(crate) fn parse_brew_outdated(output: &str) -> Vec<String> {
    // Each line is "pkgname (old-version) < new-version" or just "pkgname".
    // Extract the first whitespace-delimited token as the package name.
    output
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|l| l.split_whitespace().next().map(|s| s.to_string()))
        .collect()
}

pub(crate) fn count_homebrew_upgraded(output: &str) -> usize {
    output
        .lines()
        .filter(|l| {
            (l.contains("Upgrading") || l.contains("Pouring")) && !l.contains("outdated packages")
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_homebrew_upgraded_some() {
        let output = "==> Upgrading 2 outdated packages:\n==> Upgrading htop\n==> Pouring htop--3.3.0.arm64_sonoma.bottle.tar.gz\n";
        assert_eq!(count_homebrew_upgraded(output), 2);
    }

    #[test]
    fn test_count_homebrew_upgraded_none() {
        let output = "Already up-to-date.\n";
        assert_eq!(count_homebrew_upgraded(output), 0);
    }

    #[test]
    fn test_parse_brew_outdated_happy_path() {
        let output = "htop (3.2.2) < 3.3.0\ncurl (8.4.0) < 8.5.0\n";
        let result = parse_brew_outdated(output);
        assert_eq!(result, vec!["htop".to_string(), "curl".to_string()]);
    }

    #[test]
    fn test_parse_brew_outdated_empty() {
        assert!(parse_brew_outdated("").is_empty());
    }
}
