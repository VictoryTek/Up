use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::detect::{detect_nixos_config_type, DistroInfo, NixOsConfigType};
use super::version::next_nixos_channel;

/// Execute the actual distro upgrade.
/// Returns `Ok(())` if all upgrade steps completed successfully, or `Err(reason)` otherwise.
pub fn execute_upgrade(
    distro: &DistroInfo,
    tx: &async_channel::Sender<String>,
) -> Result<(), String> {
    let _ = tx.send_blocking(format!(
        "Starting upgrade for {} {}...",
        distro.name, distro.version
    ));

    match distro.id.as_str() {
        "ubuntu" => upgrade_ubuntu(tx),
        "fedora" => upgrade_fedora(tx),
        "opensuse-leap" => upgrade_opensuse(tx),
        "nixos" => upgrade_nixos(distro, tx),
        _ => {
            let msg = format!(
                "Upgrade is not yet supported for '{}'. Supported: Ubuntu, Fedora, openSUSE Leap, NixOS.",
                distro.name
            );
            let _ = tx.send_blocking(msg.clone());
            Err(msg)
        }
    }
}

fn upgrade_ubuntu(tx: &async_channel::Sender<String>) -> Result<(), String> {
    let _ = tx.send_blocking("Preparing Ubuntu distribution upgrade...".into());
    let _ = tx.send_blocking(
        "This operation downloads and installs many packages. It may take 30\u{2013}60 \
         minutes. Do not power off the system."
            .into(),
    );

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

    let result = if !crate::runner::run_command_sync(
        "pkexec",
        &[
            "do-release-upgrade",
            "-f",
            "DistUpgradeViewNonInteractive",
            "-e",
            "DEBIAN_FRONTEND=noninteractive",
        ],
        tx,
    ) {
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

fn upgrade_fedora(tx: &async_channel::Sender<String>) -> Result<(), String> {
    // Step 1: Ensure the system-upgrade plugin is present (best-effort; it is
    // usually pre-installed on Fedora 41+ as part of dnf5-plugins).
    let _ = tx.send_blocking("Ensuring system-upgrade plugin is installed...".into());
    // Try the DNF5 plugin name first (Fedora 41+), then the DNF4 name as fallback.
    // Failure is non-fatal because the plugin ships pre-installed on most systems.
    if !crate::runner::run_command_sync(
        "pkexec",
        &["dnf", "install", "-y", "dnf5-plugin-system-upgrade"],
        tx,
    ) {
        let _ = tx.send_blocking(
            "dnf5-plugin-system-upgrade not found; trying dnf-plugin-system-upgrade...".into(),
        );
        // Ignore failure — the plugin is typically already present.
        let _ = crate::runner::run_command_sync(
            "pkexec",
            &["dnf", "install", "-y", "dnf-plugin-system-upgrade"],
            tx,
        );
    }

    // Step 2: Download upgrade packages (next version)
    let _ = tx.send_blocking("Downloading upgrade packages...".into());

    // Detect next version
    let next_version = match detect_next_fedora_version() {
        Some(v) => v,
        None => {
            let _ = tx.send_blocking(
                "Error: Could not detect current Fedora version. Aborting upgrade.".into(),
            );
            return Err(
                "Could not detect current Fedora version to determine upgrade target".to_string(),
            );
        }
    };
    let ver_str = next_version.to_string();
    if !crate::runner::run_command_sync(
        "pkexec",
        &[
            "dnf",
            "system-upgrade",
            "download",
            "--releasever",
            &ver_str,
            "--allow-downgrade",
            "-y",
        ],
        tx,
    ) {
        return Err(format!(
            "Failed to download Fedora {} upgrade packages (see log for details)",
            next_version
        ));
    }

    // Step 3: Trigger the offline upgrade reboot.
    // `dnf system-upgrade reboot` prepares the offline transaction and immediately
    // calls `systemctl reboot`. We spawn it without waiting because:
    //   • If the reboot succeeds, systemd will kill our process via SIGTERM before
    //     the child exits, so `run_command_sync` would return false (a spurious error).
    //   • Spawning fire-and-forget lets the OS shut us down naturally while the
    //     reboot dialog gives the user a visible confirmation.
    let _ = tx.send_blocking("Download complete. Scheduling upgrade for next reboot...".into());
    use std::process::Stdio;
    let mut child = match std::process::Command::new("pkexec")
        .args(["dnf", "system-upgrade", "reboot"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => return Err(format!("Failed to start upgrade reboot: {e}")),
    };

    // Forward stdout to the log channel in a background thread.
    // This thread is naturally killed when the process is rebooted by systemd.
    if let Some(stdout) = child.stdout.take() {
        let tx_out = tx.clone();
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = tx_out.send_blocking(line);
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        let tx_err = tx.clone();
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = tx_err.send_blocking(format!("[stderr] {line}"));
            }
        });
    }

    let _ = tx.send_blocking(
        "Upgrade reboot triggered. The system will restart to apply the upgrade.".into(),
    );
    Ok(())
}

fn upgrade_opensuse(tx: &async_channel::Sender<String>) -> Result<(), String> {
    let _ = tx.send_blocking("Running zypper distribution upgrade...".into());
    if !crate::runner::run_command_sync("pkexec", &["zypper", "dup", "-y"], tx) {
        return Err(
            "openSUSE distribution upgrade command failed (see log for details)".to_string(),
        );
    }
    Ok(())
}

fn upgrade_nixos(distro: &DistroInfo, tx: &async_channel::Sender<String>) -> Result<(), String> {
    /// Colon-separated PATH prepended for NixOS tool access under pkexec.
    ///
    /// pkexec resets PATH to a minimal set, excluding NixOS-specific tool paths.
    /// We set PATH explicitly via `/usr/bin/env` to avoid a shell wrapper.
    const NIX_PATH: &str =
        "/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin";
    let config_type = detect_nixos_config_type();
    match config_type {
        NixOsConfigType::LegacyChannel => {
            let _ = tx.send_blocking("Detected: legacy channel-based NixOS config".into());

            // Determine the target channel
            let next_channel = match next_nixos_channel(&distro.version_id) {
                Some(ch) => ch,
                None => {
                    let msg = format!(
                        "Cannot determine next NixOS channel from version '{}'",
                        distro.version_id
                    );
                    let _ = tx.send_blocking(msg.clone());
                    return Err(msg);
                }
            };
            let channel_url = format!("https://nixos.org/channels/{}", next_channel);

            // Step 1: Register the new channel
            let _ = tx.send_blocking(format!("Switching channel to {}...", next_channel));
            // Pass channel_url as a positional argument; no sh -c needed.
            // /usr/bin/env sets PATH without requiring a shell.
            let path_arg = format!("PATH={}", NIX_PATH);
            if !crate::runner::run_command_sync(
                "pkexec",
                &[
                    "/usr/bin/env",
                    &path_arg,
                    "nix-channel",
                    "--add",
                    &channel_url,
                    "nixos",
                ],
                tx,
            ) {
                return Err(format!(
                    "Failed to register NixOS channel {} (see log for details)",
                    next_channel
                ));
            }

            // Step 2: Rebuild with --upgrade to apply the new channel
            let _ = tx.send_blocking(format!(
                "Rebuilding NixOS with {} (nixos-rebuild switch --upgrade)...",
                next_channel
            ));
            if !crate::runner::run_command_sync(
                "pkexec",
                &["nixos-rebuild", "switch", "--upgrade"],
                tx,
            ) {
                return Err(
                    "Failed to rebuild NixOS with --upgrade (see log for details)".to_string(),
                );
            }
            Ok(())
        }
        NixOsConfigType::Flake => {
            let _ = tx.send_blocking("Detected: flake-based NixOS config".into());
            let _ = tx.send_blocking("Updating flake inputs in /etc/nixos...".into());
            let path_arg = format!("PATH={}", NIX_PATH);
            if !crate::runner::run_command_sync(
                "pkexec",
                &[
                    "/usr/bin/env",
                    &path_arg,
                    "nix",
                    "flake",
                    "update",
                    "--flake",
                    "/etc/nixos",
                ],
                tx,
            ) {
                return Err(
                    "Failed to update flake inputs in /etc/nixos (see log for details)".to_string(),
                );
            }
            // Resolve the flake attribute name using the same mechanism as
            // NixBackend::run_update() — reads /etc/nixos/vexos-variant and
            // validates with validate_flake_attr(). This ensures both upgrade
            // paths use the same configuration attribute name.
            let config_attr = match crate::backends::nix::resolve_nixos_flake_attr() {
                Ok(attr) => attr,
                Err(e) => {
                    let msg = format!("Upgrade aborted: {e}");
                    let _ = tx.send_blocking(msg.clone());
                    return Err(msg);
                }
            };
            let flake_target = format!("/etc/nixos#{}", config_attr);
            let _ = tx.send_blocking(format!("Rebuilding NixOS configuration: {flake_target}"));
            if !crate::runner::run_command_sync(
                "pkexec",
                &["nixos-rebuild", "switch", "--flake", &flake_target],
                tx,
            ) {
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
    use super::execute_upgrade;
    use crate::upgrade::detect::DistroInfo;

    #[test]
    fn execute_upgrade_unsupported_distro_returns_err() {
        let distro = DistroInfo {
            id: "arch".to_string(),
            name: "Arch Linux".to_string(),
            version: "2026.01.01".to_string(),
            version_id: "2026".to_string(),
            upgrade_supported: false,
        };
        let (tx, _rx) = async_channel::unbounded::<String>();
        let result = execute_upgrade(&distro, &tx);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("not yet supported"),
            "unexpected message: {msg}"
        );
    }
}
