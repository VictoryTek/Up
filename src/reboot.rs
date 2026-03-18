use std::path::Path;
use std::process::Command;

/// Issue a system reboot.
/// Inside a Flatpak sandbox, tunnels through `flatpak-spawn --host` to reach
/// the host systemd. Outside Flatpak, calls `systemctl reboot` directly.
/// Uses `Command::spawn` (fire-and-forget) so the GTK loop is not blocked.
pub fn reboot() {
    if Path::new("/.flatpak-info").exists() {
        if let Err(e) = Command::new("flatpak-spawn")
            .args(["--host", "systemctl", "reboot"])
            .spawn()
        {
            eprintln!("Failed to spawn reboot command: {e}");
        }
    } else if let Err(e) = Command::new("systemctl").arg("reboot").spawn() {
        eprintln!("Failed to spawn reboot command: {e}");
    }
}
