use log::info;
use std::path::Path;
use std::process::Command;

/// Returns `true` if the system requires a reboot to complete pending updates.
///
/// Two checks are performed; either one returning `true` causes the function to
/// return `true`. Individual check failures are silently treated as `false` so
/// that a missing tool or permission error never panics the UI.
///
/// **Check 1 — Debian/Ubuntu `/var/run/reboot-required` marker.**
/// Inside a Flatpak sandbox the file is tested via `flatpak-spawn --host`.
///
/// **Check 2 — `needrestart -b` batch mode** (optional; skipped if the binary
/// is absent). A `NEEDRESTART-KSTA:` value of `2` (kernel updated, reboot
/// needed) or `3` (ABI change) is treated as a reboot requirement. Inside
/// Flatpak the command is tunnelled through `flatpak-spawn --host`.
pub fn reboot_required() -> bool {
    let in_flatpak = crate::backends::flatpak::is_running_in_flatpak();

    // Check 1: /var/run/reboot-required (Debian/Ubuntu)
    let check1 = if in_flatpak {
        Command::new("flatpak-spawn")
            .args(["--host", "test", "-f", "/var/run/reboot-required"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    } else {
        Path::new("/var/run/reboot-required").exists()
    };

    if check1 {
        return true;
    }

    // Check 2: needrestart -b (optional)
    let needrestart_output = if in_flatpak {
        // In Flatpak we cannot easily run `which` against the host PATH, so we
        // attempt the command directly; failure to spawn is treated as absence.
        Command::new("flatpak-spawn")
            .args(["--host", "needrestart", "-b"])
            .output()
            .ok()
    } else if which::which("needrestart").is_ok() {
        Command::new("needrestart").arg("-b").output().ok()
    } else {
        None
    };

    if let Some(out) = needrestart_output {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("NEEDRESTART-KSTA:") {
                let val = rest.trim();
                if val == "2" || val == "3" {
                    return true;
                }
            }
        }
    }

    false
}

/// Issue a system reboot.
/// Inside a Flatpak sandbox, tunnels through `flatpak-spawn --host` to reach
/// the host systemd. Outside Flatpak, calls `systemctl reboot` directly.
///
/// Returns `Ok(())` if the command was successfully dispatched (in practice
/// this is unreachable on success because systemd kills the process), or
/// `Err(reason)` if the reboot command itself failed.
pub fn reboot() -> Result<(), String> {
    info!("Reboot requested");
    let status = if Path::new("/.flatpak-info").exists() {
        Command::new("flatpak-spawn")
            .args(["--host", "systemctl", "reboot"])
            .status()
            .map_err(|e| format!("Failed to start reboot command: {e}"))?
    } else {
        Command::new("systemctl")
            .arg("reboot")
            .status()
            .map_err(|e| format!("Failed to start reboot command: {e}"))?
    };
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Reboot command exited with code {:?}",
            status.code()
        ))
    }
}
