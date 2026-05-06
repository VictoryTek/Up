use log::info;
use std::path::Path;
use std::process::Command;

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
