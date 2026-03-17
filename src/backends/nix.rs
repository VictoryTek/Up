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
                // Export the NixOS binary paths explicitly: pkexec resets PATH
                // to standard directories that typically do not include Nix
                // tooling on NixOS.
                let cmd = format!(
                    "export PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin:$PATH && cd /etc/nixos && nix flake update && nixos-rebuild switch --flake /etc/nixos#{}",
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
                // Copy flake.nix (and lock if present) to a temp dir and run
                // `nix flake update` there.  This avoids needing root and does
                // not touch /etc/nixos, while still fetching the latest input
                // revisions from the network to produce an accurate count.
                let tmpdir = std::env::temp_dir().join("up-nix-check");
                let _ = tokio::fs::remove_dir_all(&tmpdir).await;
                if tokio::fs::create_dir_all(&tmpdir).await.is_err()
                    || tokio::fs::copy("/etc/nixos/flake.nix", tmpdir.join("flake.nix"))
                        .await
                        .is_err()
                {
                    return Err("Cannot read /etc/nixos/flake.nix".to_string());
                }
                // Bring the existing lock so nix can diff against it.
                let _ =
                    tokio::fs::copy("/etc/nixos/flake.lock", tmpdir.join("flake.lock")).await;
                let result = tokio::process::Command::new("nix")
                    .args(["flake", "update"])
                    .current_dir(&tmpdir)
                    .output()
                    .await;
                let _ = tokio::fs::remove_dir_all(&tmpdir).await;
                match result {
                    Ok(out) => {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        let count = stderr
                            .lines()
                            .filter(|l| l.contains("Updated input"))
                            .count();
                        Ok(count)
                    }
                    Err(e) => Err(format!("nix: {e}")),
                }
            } else {
                // Legacy NixOS channels have no unprivileged check mechanism.
                Err("Click Update All to check".to_string())
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
