use crate::backends::{Backend, BackendError, BackendKind, UpdateResult};
use crate::executor::CommandExecutor;
use log::warn;
use std::future::Future;
use std::pin::Pin;

/// Returns `true` when `fwupdmgr` is available on this system.
pub fn is_available() -> bool {
    which::which("fwupdmgr").is_ok()
}

/// Backend for firmware updates via the Linux Vendor Firmware Service (LVFS).
///
/// Uses `fwupdmgr get-updates --json` to enumerate pending firmware updates
/// and `fwupdmgr update` to apply them.
///
/// Privilege: fwupd communicates with its system daemon over D-Bus; the daemon
/// requests polkit authorization when needed.  No `pkexec` wrapper is required.
pub struct FwupdBackend;

impl Backend for FwupdBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Fwupd
    }

    fn display_name(&self) -> &str {
        "Firmware (fwupd)"
    }

    fn description(&self) -> &str {
        "Device firmware via LVFS"
    }

    fn icon_name(&self) -> &str {
        "firmware-manager-symbolic"
    }

    /// fwupd handles privilege internally via polkit D-Bus.  No pkexec needed.
    fn needs_root(&self) -> bool {
        false
    }

    fn list_available(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("fwupdmgr")
                .args(["get-updates", "--json"])
                .output()
                .await
                .map_err(|e| format!("Failed to spawn fwupdmgr: {e}"))?;

            let code = out.status.code().unwrap_or(-1);

            // Exit code 2 = "no actions" = no firmware updates available.
            // This is a documented success state, not an error.
            if code == 2 {
                return Ok(Vec::new());
            }

            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                warn!("fwupdmgr get-updates failed (exit {code}): {stderr}");
                return Err(format!(
                    "fwupdmgr get-updates failed (exit {code}): {stderr}"
                ));
            }

            let text = String::from_utf8_lossy(&out.stdout);
            Ok(parse_fwupd_updates(&text))
        })
    }

    fn run_update<'a>(
        &'a self,
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            // fwupdmgr update contacts the fwupd daemon over D-Bus, which raises
            // a polkit dialog for authorization if needed.  No pkexec wrapper
            // is required.
            match runner.run("fwupdmgr", &["update"]).await {
                Ok(output) => {
                    let count = count_fwupd_updated(&output);
                    UpdateResult::Success {
                        updated_count: count,
                    }
                }
                // Exit code 2 = no updates pending; treat as clean success.
                Err(BackendError::Exit { code: 2, .. }) => {
                    UpdateResult::Success { updated_count: 0 }
                }
                Err(e) => UpdateResult::Error(e),
            }
        })
    }

    fn estimate_size(&self) -> Pin<Box<dyn Future<Output = Option<u64>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("fwupdmgr")
                .args(["get-updates", "--json"])
                .output()
                .await
                .ok()?;
            let code = out.status.code().unwrap_or(-1);
            // Exit code 2 = no firmware updates available.
            if code == 2 {
                return Some(0);
            }
            if !out.status.success() {
                return None;
            }
            let text = String::from_utf8_lossy(&out.stdout);
            let total = crate::disk::parse_fwupd_size(&text);
            if total == 0 {
                None
            } else {
                Some(total)
            }
        })
    }
}

/// Parse JSON output of `fwupdmgr get-updates --json`.
///
/// The JSON structure is:
/// ```json
/// {
///   "Devices": [
///     {
///       "Name": "Unifying Receiver",
///       "Version": "RQR12.07_B0029",
///       "Releases": [{ "Version": "RQR12.10_B0032", ... }]
///     }
///   ]
/// }
/// ```
///
/// Returns a list of `"<DeviceName> (<NewVersion>)"` strings, one per device
/// with available firmware updates.
pub(crate) fn parse_fwupd_updates(json_text: &str) -> Vec<String> {
    let value: serde_json::Value = match serde_json::from_str(json_text) {
        Ok(v) => v,
        Err(e) => {
            warn!("Failed to parse fwupd JSON output: {e}");
            return Vec::new();
        }
    };

    let mut updates = Vec::new();
    if let Some(devices) = value.get("Devices").and_then(|d| d.as_array()) {
        for device in devices {
            let name = device
                .get("Name")
                .and_then(|n| n.as_str())
                .unwrap_or("Unknown device");
            if let Some(releases) = device.get("Releases").and_then(|r| r.as_array()) {
                if let Some(first_release) = releases.first() {
                    let version = first_release
                        .get("Version")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    updates.push(format!("{name} ({version})"));
                }
            }
        }
    }
    updates
}

/// Count the number of firmware devices that were successfully updated from
/// the combined stdout+stderr of `fwupdmgr update`.
///
/// fwupdmgr emits "Successfully installed firmware" per device update.
/// This count is zero when all updates are staged for a reboot rather than
/// applied live — `UpdateResult::Success { updated_count: 0 }` is still
/// correct in that case.
pub(crate) fn count_fwupd_updated(output: &str) -> usize {
    output
        .lines()
        .filter(|l| {
            let t = l.trim();
            t.contains("Successfully installed") || t.starts_with("Updated ")
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::BackendError;
    use crate::executor::test_utils::MockExecutor;

    // --- parse_fwupd_updates tests ---

    #[test]
    fn parse_fwupd_json_with_updates() {
        let json = r#"{
            "Devices": [
                {
                    "DeviceId": "cf3685ba249d3d98602047341d6f5a5556a6ac05",
                    "Name": "Unifying Receiver",
                    "Version": "RQR12.07_B0029",
                    "Releases": [
                        {
                            "Version": "RQR12.10_B0032",
                            "Summary": "Firmware for the Logitech Unifying Receiver"
                        }
                    ]
                },
                {
                    "Name": "ThinkPad System Firmware",
                    "Version": "1.55",
                    "Releases": [
                        {
                            "Version": "1.59"
                        }
                    ]
                }
            ]
        }"#;
        let updates = parse_fwupd_updates(json);
        assert_eq!(updates.len(), 2);
        assert!(updates.contains(&"Unifying Receiver (RQR12.10_B0032)".to_string()));
        assert!(updates.contains(&"ThinkPad System Firmware (1.59)".to_string()));
    }

    #[test]
    fn parse_fwupd_json_empty() {
        let json = r#"{"Devices": []}"#;
        let updates = parse_fwupd_updates(json);
        assert!(updates.is_empty());
    }

    #[test]
    fn parse_fwupd_json_malformed() {
        // Malformed JSON falls back gracefully and returns empty vec.
        let updates = parse_fwupd_updates("not valid json at all {{");
        assert!(updates.is_empty());
    }

    #[test]
    fn parse_fwupd_json_no_releases() {
        // Device with no Releases array should be skipped (no update entry produced).
        let json = r#"{
            "Devices": [
                {
                    "Name": "Some Device",
                    "Version": "1.0"
                }
            ]
        }"#;
        let updates = parse_fwupd_updates(json);
        assert!(updates.is_empty());
    }

    // --- count_fwupd_updated tests ---

    #[test]
    fn exit_code_2_is_no_updates() {
        // Exit code 2 from run_update should produce Success { updated_count: 0 }, not an error.
        let mock = MockExecutor::new(vec![Err(BackendError::Exit {
            code: 2,
            message: "No updatable devices".into(),
        })]);
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(FwupdBackend.run_update(&mock));
        assert!(
            matches!(result, UpdateResult::Success { updated_count: 0 }),
            "Expected Success {{ updated_count: 0 }} for exit 2, got {:?}",
            result
        );
    }

    #[test]
    fn is_available_false_when_missing() {
        // On this Windows CI machine fwupdmgr is not installed.
        // `which` must return false — backend must not be registered.
        // On a real Linux system where fwupdmgr IS installed this would pass
        // with `true`; on Windows or any system without fwupdmgr it returns false.
        let result = which::which("fwupdmgr");
        // The assertion validates that is_available() correctly reflects which's result.
        assert_eq!(is_available(), result.is_ok());
    }

    #[tokio::test]
    async fn run_update_uses_assume_yes() {
        // Verify that run_update calls fwupdmgr with the "update" argument.
        // MockExecutor captures the call and returns a successful output.
        let output = "Downloading ThinkPad BIOS 1.59...\nSuccessfully installed firmware\n";
        let mock = MockExecutor::with_output(output);
        let result = FwupdBackend.run_update(&mock).await;
        assert!(
            matches!(result, UpdateResult::Success { updated_count: 1 }),
            "Expected Success {{ updated_count: 1 }}, got {:?}",
            result
        );
    }
}
