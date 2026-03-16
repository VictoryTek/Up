use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::runner::CommandRunner;

pub fn is_available() -> bool {
    which::which("nix").is_ok()
}

/// True when running on NixOS (the /etc/nixos directory is present).
fn is_nixos() -> bool {
    std::path::Path::new("/etc/nixos").exists()
}

/// True when the NixOS config is flake-based (/etc/nixos/flake.nix exists).
fn is_nixos_flake() -> bool {
    std::path::Path::new("/etc/nixos/flake.nix").exists()
}

fn nixos_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .unwrap_or_else(|_| "nixos".to_owned())
        .trim()
        .to_string()
}

pub struct NixBackend;

#[async_trait::async_trait]
impl Backend for NixBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Nix
    }

    fn display_name(&self) -> &str {
        "Nix"
    }

    fn description(&self) -> &str {
        if is_nixos() {
            "NixOS system packages"
        } else {
            "Nix profile packages"
        }
    }

    fn icon_name(&self) -> &str {
        "system-software-install-symbolic"
    }

    async fn run_update(&self, runner: &CommandRunner) -> UpdateResult {
        if is_nixos() {
            if is_nixos_flake() {
                // Flake-based NixOS: update the flake inputs then rebuild.
                let hostname = nixos_hostname();
                let cmd = format!(
                    "nix flake update /etc/nixos && nixos-rebuild switch --flake /etc/nixos#{}",
                    hostname
                );
                match runner.run("pkexec", &["sh", "-c", &cmd]).await {
                    Ok(output) => UpdateResult::Success {
                        updated_count: output.lines().filter(|l| !l.is_empty()).count(),
                    },
                    Err(e) => UpdateResult::Error(e),
                }
            } else {
                // Legacy NixOS channels.
                match runner
                    .run("pkexec", &["nixos-rebuild", "switch", "--upgrade"])
                    .await
                {
                    Ok(output) => UpdateResult::Success {
                        updated_count: output.lines().filter(|l| !l.is_empty()).count(),
                    },
                    Err(e) => UpdateResult::Error(e),
                }
            }
        } else {
            // Non-NixOS: update the user's nix profile.
            let use_flakes = runner.run("nix", &["profile", "list"]).await.is_ok();
            if use_flakes {
                match runner.run("nix", &["profile", "upgrade", ".*"]).await {
                    Ok(output) => UpdateResult::Success {
                        updated_count: output.lines().filter(|l| !l.is_empty()).count(),
                    },
                    Err(e) => UpdateResult::Error(e),
                }
            } else {
                match runner.run("nix-env", &["-u"]).await {
                    Ok(output) => UpdateResult::Success {
                        updated_count: output.lines().filter(|l| l.contains("upgrading")).count(),
                    },
                    Err(e) => UpdateResult::Error(e),
                }
            }
        }
    }

    async fn count_available(&self) -> Result<usize, String> {
        if is_nixos() {
            if is_nixos_flake() {
                // nix flake update --dry-run (Nix >= 2.19) checks for available input
                // updates without writing the lock file.
                let out = tokio::process::Command::new("nix")
                    .args(["flake", "update", "--dry-run", "/etc/nixos"])
                    .output()
                    .await
                    .map_err(|e| e.to_string())?;
                if out.status.success() {
                    let text = String::from_utf8_lossy(&out.stderr);
                    Ok(text.lines().filter(|l| l.contains("Updated input")).count())
                } else {
                    // Older Nix without --dry-run support.
                    Err("Run update to check".to_string())
                }
            } else {
                // Legacy NixOS channels have no dry-run check mechanism.
                Err("Run update to check".to_string())
            }
        } else {
            let out = tokio::process::Command::new("nix-env")
                .args(["-u", "--dry-run"])
                .output()
                .await
                .map_err(|e| e.to_string())?;
            // nix-env --dry-run writes "upgrading ..." lines to stderr
            let text = String::from_utf8_lossy(&out.stderr);
            Ok(text.lines().filter(|l| l.contains("upgrading")).count())
        }
    }
}
