use std::path::Path;
use thiserror::Error;
use which::which;

/// The snapshot tool to use for pre-update snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotTool {
    Snapper,
    Timeshift,
    Btrfs,
}

/// Errors that can occur during snapshot creation.
#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("Snapshot command failed (exit {0}): {1}")]
    Exit(i32, String),
    #[error("Failed to spawn snapshot command: {0}")]
    Spawn(#[from] std::io::Error),
}

/// Detect the highest-priority available snapshot tool.
///
/// Priority: Snapper > Timeshift > Btrfs
pub fn detect_snapshot_tool() -> Option<SnapshotTool> {
    if which("snapper").is_ok() && Path::new("/etc/snapper/configs/root").exists() {
        return Some(SnapshotTool::Snapper);
    }
    if which("timeshift").is_ok() && Path::new("/etc/timeshift/timeshift.json").exists() {
        return Some(SnapshotTool::Timeshift);
    }
    if is_root_btrfs() && Path::new("/.snapshots").exists() {
        return Some(SnapshotTool::Btrfs);
    }
    None
}

fn is_root_btrfs() -> bool {
    std::fs::read_to_string("/proc/mounts")
        .ok()
        .map(|content| {
            content.lines().any(|line| {
                let parts: Vec<&str> = line.split_whitespace().collect();
                parts.len() >= 3 && parts[1] == "/" && parts[2] == "btrfs"
            })
        })
        .unwrap_or(false)
}

/// Create a pre-update snapshot using the given tool.
///
/// Runs `pkexec <tool-command>` as a one-shot privileged process.
/// Returns a human-readable description of the created snapshot on success.
pub async fn create_snapshot(tool: SnapshotTool) -> Result<String, SnapshotError> {
    match tool {
        SnapshotTool::Snapper => {
            let output = tokio::process::Command::new("pkexec")
                .arg("snapper")
                .arg("-c")
                .arg("root")
                .arg("create")
                .arg("-t")
                .arg("pre")
                .arg("--print-number")
                .arg("--description")
                .arg("Up pre-update")
                .output()
                .await?;
            if output.status.success() {
                let number = String::from_utf8_lossy(&output.stdout).trim().to_string();
                Ok(format!("Snapper snapshot #{number}"))
            } else {
                let code = output.status.code().unwrap_or(-1);
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                Err(SnapshotError::Exit(code, stderr))
            }
        }
        SnapshotTool::Timeshift => {
            let output = tokio::process::Command::new("pkexec")
                .arg("timeshift")
                .arg("--create")
                .arg("--comments")
                .arg("Up pre-update")
                .arg("--scripted")
                .output()
                .await?;
            if output.status.success() {
                Ok("Timeshift snapshot created".to_string())
            } else {
                let code = output.status.code().unwrap_or(-1);
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                Err(SnapshotError::Exit(code, stderr))
            }
        }
        SnapshotTool::Btrfs => {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let dest = format!("/.snapshots/pre-update-{ts}");
            let output = tokio::process::Command::new("pkexec")
                .arg("btrfs")
                .arg("subvolume")
                .arg("snapshot")
                .arg("/")
                .arg(&dest)
                .output()
                .await?;
            if output.status.success() {
                Ok(format!("btrfs snapshot at {dest}"))
            } else {
                let code = output.status.code().unwrap_or(-1);
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                Err(SnapshotError::Exit(code, stderr))
            }
        }
    }
}
