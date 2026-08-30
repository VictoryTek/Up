use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::detect::{detect_nixos_config_type, DistroInfo, NixOsConfigType, UpgradeStrategy};
use super::version::next_nixos_channel;
use crate::backends::BackendKind;
use crate::executor::CommandExecutor;
use crate::runner::{BackendEvent, CommandRunner, PrivilegedShell};

/// Map a distro id to the `BackendKind` used to tag upgrade log events.
/// Purely cosmetic — no new `BackendKind` variant is introduced for upgrades.
fn upgrade_kind(distro_id: &str) -> BackendKind {
    match UpgradeStrategy::for_distro(distro_id) {
        Some(UpgradeStrategy::Fedora) => BackendKind::Dnf,
        Some(UpgradeStrategy::OpenSuseLeap) => BackendKind::Zypper,
        Some(UpgradeStrategy::NixOs) => BackendKind::Nix,
        Some(UpgradeStrategy::Ubuntu) | None => BackendKind::Apt,
    }
}

/// Entry point for the distro upgrade workflow.
///
/// Authenticates **once** via [`PrivilegedShell`], then runs every upgrade step
/// through a shared [`CommandRunner`] (one polkit prompt for the whole
/// upgrade). Command output and narrative lines are streamed to `log_tx` in
/// order. Returns `Ok(())` when all steps complete, `Err(reason)` otherwise.
pub async fn run_upgrade(
    distro: &DistroInfo,
    log_tx: &async_channel::Sender<String>,
) -> Result<(), String> {
    let shell = match PrivilegedShell::new().await {
        Ok(s) => Arc::new(tokio::sync::Mutex::new(s)),
        Err(e) => return Err(format!("Authentication failed: {e}")),
    };

    // Relay the runner's streamed command output into the same log channel.
    let (be_tx, be_rx) = async_channel::unbounded::<BackendEvent>();
    let log_tx_fwd = log_tx.clone();
    let fwd_handle = tokio::spawn(async move {
        while let Ok(BackendEvent::LogLine(_, line)) = be_rx.recv().await {
            let _ = log_tx_fwd.send(line).await;
        }
    });

    let runner = CommandRunner::new(be_tx.clone(), upgrade_kind(&distro.id), Some(shell.clone()));
    let result = execute_upgrade(distro, log_tx, &runner).await;

    drop(be_tx);
    let _ = fwd_handle.await;
    shell.lock().await.close().await;

    result
}

/// Execute the actual distro upgrade steps through `runner`.
/// Returns `Ok(())` if all upgrade steps completed successfully, or `Err(reason)` otherwise.
pub(crate) async fn execute_upgrade(
    distro: &DistroInfo,
    tx: &async_channel::Sender<String>,
    runner: &dyn CommandExecutor,
) -> Result<(), String> {
    let _ = tx
        .send(format!(
            "Starting upgrade for {} {}...",
            distro.name, distro.version
        ))
        .await;

    match UpgradeStrategy::for_distro(&distro.id) {
        Some(UpgradeStrategy::Ubuntu) => upgrade_ubuntu(tx, runner).await,
        Some(UpgradeStrategy::Fedora) => upgrade_fedora(tx, runner).await,
        Some(UpgradeStrategy::OpenSuseLeap) => upgrade_opensuse(tx, runner).await,
        Some(UpgradeStrategy::NixOs) => upgrade_nixos(distro, tx, runner).await,
        None => {
            let msg = format!(
                "Upgrade is not yet supported for '{}'. Supported: Ubuntu, Fedora, openSUSE Leap, NixOS.",
                distro.name
            );
            let _ = tx.send(msg.clone()).await;
            Err(msg)
        }
    }
}

/// Run one privileged upgrade step. Output is streamed by `runner`; the return
/// value is `true` on exit code 0.
async fn run_step(runner: &dyn CommandExecutor, args: &[&str]) -> bool {
    runner.run("pkexec", args).await.is_ok()
}

async fn upgrade_ubuntu(
    tx: &async_channel::Sender<String>,
    runner: &dyn CommandExecutor,
) -> Result<(), String> {
    let _ = tx
        .send("Preparing Ubuntu distribution upgrade...".into())
        .await;
    let _ = tx
        .send(
            "This operation downloads and installs many packages. It may take 30\u{2013}60 \
             minutes. Do not power off the system."
                .into(),
        )
        .await;

    let log_path = "/var/log/dist-upgrade/main.log";
    let tx_tail = tx.clone();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let cancel_flag_thread = Arc::clone(&cancel_flag);
    let tail_handle = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        use std::io::{BufRead, BufReader, Seek, SeekFrom};
        let Ok(mut file) = std::fs::File::open(log_path) else {
            return;
        };
        let _ = file.seek(SeekFrom::End(0));
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        loop {
            if cancel_flag_thread.load(Ordering::Relaxed) {
                break;
            }
            match reader.read_line(&mut line) {
                Ok(0) => {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                Ok(_) => {
                    let trimmed = line.trim_end_matches('\n').to_string();
                    if !trimmed.is_empty() {
                        let _ = tx_tail.send_blocking(format!("[log] {}", trimmed));
                    }
                    line.clear();
                }
                Err(_) => break,
            }
        }
    });

    let result = if !run_step(
        runner,
        &[
            "do-release-upgrade",
            "-f",
            "DistUpgradeViewNonInteractive",
            "-e",
            "DEBIAN_FRONTEND=noninteractive",
        ],
    )
    .await
    {
        Err("Ubuntu distribution upgrade failed (see log for details)".to_string())
    } else {
        Ok(())
    };

    // Set cancellation flag so the tail thread exits its loop.
    cancel_flag.store(true, Ordering::Relaxed);
    // Wait for the tail thread to finish draining any remaining lines.
    let _ = tail_handle.join();
    result
}

async fn upgrade_fedora(
    tx: &async_channel::Sender<String>,
    runner: &dyn CommandExecutor,
) -> Result<(), String> {
    // Step 1: Ensure the system-upgrade plugin is present (best-effort; it is
    // usually pre-installed on Fedora 41+ as part of dnf5-plugins).
    let _ = tx
        .send("Ensuring system-upgrade plugin is installed...".into())
        .await;
    // Try the DNF5 plugin name first (Fedora 41+), then the DNF4 name as fallback.
    // Failure is non-fatal because the plugin ships pre-installed on most systems.
    if !run_step(
        runner,
        &["dnf", "install", "-y", "dnf5-plugin-system-upgrade"],
    )
    .await
    {
        let _ = tx
            .send(
                "dnf5-plugin-system-upgrade not found; trying dnf-plugin-system-upgrade...".into(),
            )
            .await;
        // Ignore failure — the plugin is typically already present.
        let _ = run_step(
            runner,
            &["dnf", "install", "-y", "dnf-plugin-system-upgrade"],
        )
        .await;
    }

    // Step 2: Download upgrade packages (next version)
    let _ = tx.send("Downloading upgrade packages...".into()).await;

    // Detect next version
    let next_version = match detect_next_fedora_version() {
        Some(v) => v,
        None => {
            let _ = tx
                .send("Error: Could not detect current Fedora version. Aborting upgrade.".into())
                .await;
            return Err(
                "Could not detect current Fedora version to determine upgrade target".to_string(),
            );
        }
    };
    let ver_str = next_version.to_string();
    if !run_step(
        runner,
        &[
            "dnf",
            "system-upgrade",
            "download",
            "--releasever",
            &ver_str,
            "--allow-downgrade",
            "-y",
        ],
    )
    .await
    {
        return Err(format!(
            "Failed to download Fedora {} upgrade packages (see log for details)",
            next_version
        ));
    }

    // Step 3: Trigger the offline upgrade reboot.
    // `dnf system-upgrade reboot` prepares the offline transaction and immediately
    // calls `systemctl reboot`, so systemd SIGTERMs this process (and the privileged
    // shell) before the command returns. A non-Ok result here is therefore expected
    // and not treated as a failure.
    let _ = tx
        .send("Download complete. Scheduling upgrade for next reboot...".into())
        .await;
    let _ = runner
        .run("pkexec", &["dnf", "system-upgrade", "reboot"])
        .await;

    let _ = tx
        .send("Upgrade reboot triggered. The system will restart to apply the upgrade.".into())
        .await;
    Ok(())
}

async fn upgrade_opensuse(
    tx: &async_channel::Sender<String>,
    runner: &dyn CommandExecutor,
) -> Result<(), String> {
    let _ = tx
        .send("Running zypper distribution upgrade...".into())
        .await;
    if !run_step(runner, &["zypper", "dup", "-y"]).await {
        return Err(
            "openSUSE distribution upgrade command failed (see log for details)".to_string(),
        );
    }
    Ok(())
}

async fn upgrade_nixos(
    distro: &DistroInfo,
    tx: &async_channel::Sender<String>,
    runner: &dyn CommandExecutor,
) -> Result<(), String> {
    /// Colon-separated PATH prepended for NixOS tool access under pkexec.
    ///
    /// pkexec resets PATH to a minimal set, excluding NixOS-specific tool paths.
    /// We set PATH explicitly via `/usr/bin/env` to avoid a shell wrapper.
    const NIX_PATH: &str =
        "/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin";
    let config_type = detect_nixos_config_type();
    match config_type {
        NixOsConfigType::LegacyChannel => {
            let _ = tx
                .send("Detected: legacy channel-based NixOS config".into())
                .await;

            // Determine the target channel
            let next_channel = match next_nixos_channel(&distro.version_id) {
                Some(ch) => ch,
                None => {
                    let msg = format!(
                        "Cannot determine next NixOS channel from version '{}'",
                        distro.version_id
                    );
                    let _ = tx.send(msg.clone()).await;
                    return Err(msg);
                }
            };
            let channel_url = format!("https://nixos.org/channels/{}", next_channel);

            // Step 1: Register the new channel
            let _ = tx
                .send(format!("Switching channel to {}...", next_channel))
                .await;
            // Pass channel_url as a positional argument; no sh -c needed.
            // /usr/bin/env sets PATH without requiring a shell.
            let path_arg = format!("PATH={}", NIX_PATH);
            if !run_step(
                runner,
                &[
                    "/usr/bin/env",
                    &path_arg,
                    "nix-channel",
                    "--add",
                    &channel_url,
                    "nixos",
                ],
            )
            .await
            {
                return Err(format!(
                    "Failed to register NixOS channel {} (see log for details)",
                    next_channel
                ));
            }

            // Step 2: Rebuild with --upgrade to apply the new channel
            let _ = tx
                .send(format!(
                    "Rebuilding NixOS with {} (nixos-rebuild switch --upgrade)...",
                    next_channel
                ))
                .await;
            if !run_step(runner, &["nixos-rebuild", "switch", "--upgrade"]).await {
                return Err(
                    "Failed to rebuild NixOS with --upgrade (see log for details)".to_string(),
                );
            }
            Ok(())
        }
        NixOsConfigType::Flake => {
            let _ = tx.send("Detected: flake-based NixOS config".into()).await;
            let _ = tx
                .send("Updating flake inputs in /etc/nixos...".into())
                .await;
            let path_arg = format!("PATH={}", NIX_PATH);
            if !run_step(
                runner,
                &[
                    "/usr/bin/env",
                    &path_arg,
                    "nix",
                    "flake",
                    "update",
                    "--flake",
                    "/etc/nixos",
                ],
            )
            .await
            {
                return Err(
                    "Failed to update flake inputs in /etc/nixos (see log for details)".to_string(),
                );
            }
            // Resolve the flake attribute name using the same mechanism as
            // NixBackend::run_update() — reads /etc/nixos/vexos-variant or
            // auto-detects from the flake, validated with validate_flake_attr().
            let config_attr = match crate::backends::nix::resolve_nixos_flake_attr() {
                Ok(attr) => attr,
                Err(e) => {
                    let msg = format!("Upgrade aborted: {e}");
                    let _ = tx.send(msg.clone()).await;
                    return Err(msg);
                }
            };
            let flake_target = format!("/etc/nixos#{}", config_attr);
            let _ = tx
                .send(format!("Rebuilding NixOS configuration: {flake_target}"))
                .await;
            if !run_step(
                runner,
                &["nixos-rebuild", "switch", "--flake", &flake_target],
            )
            .await
            {
                return Err(format!(
                    "Failed to rebuild NixOS flake configuration '{}' (see log for details)",
                    flake_target
                ));
            }
            Ok(())
        }
    }
}

fn detect_next_fedora_version() -> Option<u32> {
    // Primary: rpm macro (most accurate on Fedora)
    if let Ok(output) = Command::new("rpm").args(["-E", "%fedora"]).output() {
        let s = String::from_utf8_lossy(&output.stdout);
        let trimmed = s.trim();
        // Only accept it if it looks like a plain number (not the unexpanded macro "%fedora")
        if !trimmed.starts_with('%') {
            if let Ok(n) = trimmed.parse::<u32>() {
                return Some(n + 1);
            }
        }
    }
    // Fallback: parse VERSION_ID from /etc/os-release
    if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
        for line in content.lines() {
            if let Some(val) = line.strip_prefix("VERSION_ID=") {
                let val = val.trim_matches('"');
                if let Ok(n) = val.parse::<u32>() {
                    return Some(n + 1);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{execute_upgrade, upgrade_kind};
    use crate::backends::BackendKind;
    use crate::executor::test_utils::MockExecutor;
    use crate::upgrade::detect::DistroInfo;

    #[tokio::test]
    async fn execute_upgrade_unsupported_distro_returns_err() {
        let distro = DistroInfo {
            id: "arch".to_string(),
            name: "Arch Linux".to_string(),
            version: "2026.01.01".to_string(),
            version_id: "2026".to_string(),
            upgrade_supported: false,
        };
        let (tx, _rx) = async_channel::unbounded::<String>();
        // No responses queued: the unsupported-distro path must not touch the runner.
        let runner = MockExecutor::new(vec![]);
        let result = execute_upgrade(&distro, &tx, &runner).await;
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("not yet supported"),
            "unexpected message: {msg}"
        );
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn upgrade_kind_maps_known_distros() {
        assert_eq!(upgrade_kind("fedora"), BackendKind::Dnf);
        assert_eq!(upgrade_kind("opensuse-leap"), BackendKind::Zypper);
        assert_eq!(upgrade_kind("nixos"), BackendKind::Nix);
        assert_eq!(upgrade_kind("ubuntu"), BackendKind::Apt);
        assert_eq!(upgrade_kind("linuxmint"), BackendKind::Apt);
    }
}
