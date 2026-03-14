use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::runner::CommandRunner;

pub fn is_available() -> bool {
    which::which("nix").is_ok()
}

pub struct NixBackend;

impl Backend for NixBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Nix
    }
    fn display_name(&self) -> &str {
        "Nix"
    }
    fn description(&self) -> &str {
        "Nix profile packages"
    }
    fn icon_name(&self) -> &str {
        "system-software-install-symbolic"
    }

    async fn run_update(&self, runner: &CommandRunner) -> UpdateResult {
        // Update the default nix profile
        // For flake-based nix, use `nix profile upgrade '.*'`
        // For legacy nix, use `nix-env -u`
        let use_flakes = runner.run("nix", &["profile", "list"]).await.is_ok();

        if use_flakes {
            match runner
                .run("nix", &["profile", "upgrade", ".*"])
                .await
            {
                Ok(output) => {
                    let count = output.lines().filter(|l| !l.is_empty()).count();
                    UpdateResult::Success {
                        updated_count: count,
                    }
                }
                Err(e) => UpdateResult::Error(e),
            }
        } else {
            match runner.run("nix-env", &["-u"]).await {
                Ok(output) => {
                    let count = output
                        .lines()
                        .filter(|l| l.contains("upgrading"))
                        .count();
                    UpdateResult::Success {
                        updated_count: count,
                    }
                }
                Err(e) => UpdateResult::Error(e),
            }
        }
    }
}
