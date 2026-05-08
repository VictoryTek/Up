# fwupd Firmware Backend — Implementation Specification

**Feature:** `fwupdmgr get-updates` / `fwupdmgr update` as a new `Backend` impl  
**Spec path:** `.github/docs/subagent_docs/fwupd_backend_spec.md`  
**Date:** 2026-05-08  

---

## Research Sources

1. **fwupdmgr man page (Arch Linux)** — https://man.archlinux.org/man/fwupdmgr.1.en  
   Exit codes: 0=success, 1=generic failure, **2=no actions (no updates)**, 3=not found.  
   `--json` flag: officially documented for stable parsing output.

2. **fwupd GitHub README** — https://github.com/fwupd/fwupd  
   Basic usage: `fwupdmgr refresh` then `fwupdmgr get-updates` then `fwupdmgr update`.  
   Privilege: fwupd daemon uses polkit D-Bus internally.

3. **Arch Wiki — fwupd** — https://wiki.archlinux.org/title/Fwupd  
   Binary name is always `fwupdmgr`. Update uses polkit internally.

4. **fwupd bash completion** — https://github.com/fwupd/fwupd/blob/main/data/bash-completion/fwupdmgr  
   Confirms `--json` flag, JSON structure (`.Devices[].DeviceId`, `.Releases[].Version`).

5. **fwupd GitHub issue #1698** — https://github.com/fwupd/fwupd/issues/1698  
   Shows actual `fwupdmgr get-updates` text output format (tree structure with device names, versions, release notes).

6. **GNOME Software / fwupd integration pattern**  
   GNOME Software uses libfwupd D-Bus API; for CLI tools, `fwupdmgr get-updates --json` is the correct stable parsing interface.

7. **LVFS docs** — https://lvfs.readthedocs.io  
   Confirms JSON structure and metadata model.

8. **Existing Up codebase** — `src/backends/`, `src/executor.rs`, `src/runner.rs`, `src/backends/mod.rs`  
   Fully read and analysed.

---

## 1. Current State Analysis

### 1.1 Backend Trait (verbatim from `src/backends/mod.rs`)

```rust
pub trait Backend: Send + Sync {
    fn kind(&self) -> BackendKind;
    fn display_name(&self) -> &str;
    fn description(&self) -> &str;
    fn icon_name(&self) -> &str;

    fn run_update<'a>(
        &'a self,
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>>;

    /// Whether this backend requires root privileges (pkexec) to perform updates.
    fn needs_root(&self) -> bool {
        false
    }

    /// Count packages available for update (read-only, no privilege required).
    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async move { self.list_available().await.map(|v| v.len()) })
    }

    /// Return a human-readable list of package names pending update.
    fn list_available(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// Whether this backend supports a cleanup / maintenance operation.
    fn supports_cleanup(&self) -> bool {
        false
    }

    /// Run the cleanup/maintenance operation for this backend.
    fn run_cleanup<'a>(
        &'a self,
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        let _ = runner;
        Box::pin(async { UpdateResult::Success { updated_count: 0 } })
    }
}
```

### 1.2 BackendKind Enum (verbatim from `src/backends/mod.rs`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendKind {
    Apt,
    Dnf,
    Pacman,
    Zypper,
    Flatpak,
    Homebrew,
    Nix,
}

impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Apt => write!(f, "APT"),
            Self::Dnf => write!(f, "DNF"),
            Self::Pacman => write!(f, "Pacman"),
            Self::Zypper => write!(f, "Zypper"),
            Self::Flatpak => write!(f, "Flatpak"),
            Self::Homebrew => write!(f, "Homebrew"),
            Self::Nix => write!(f, "Nix"),
        }
    }
}
```

### 1.3 UpdateResult Enum (verbatim from `src/backends/mod.rs`)

```rust
#[derive(Debug, Clone)]
pub enum UpdateResult {
    Success {
        updated_count: usize,
    },
    SuccessWithSelfUpdate {
        updated_count: usize,
    },
    Error(BackendError),
    #[allow(dead_code)]
    Skipped(String),
    Cancelled,
}
```

### 1.4 BackendError Enum (relevant variants)

```rust
#[derive(Debug, thiserror::Error, Clone)]
pub enum BackendError {
    #[error("Authentication cancelled or denied")]
    AuthCancelled,
    #[error("Failed to spawn process: {0}")]
    Spawn(String),
    #[error("Command failed (exit {code}): {message}")]
    Exit { code: i32, message: String },
    #[error("Failed to parse command output: {0}")]
    Parse(String),
    // ...
}
```

### 1.5 CommandExecutor Trait (verbatim from `src/executor.rs`)

```rust
pub trait CommandExecutor: Send + Sync {
    fn run<'a>(
        &'a self,
        program: &'a str,
        args: &'a [&'a str],
    ) -> Pin<Box<dyn Future<Output = Result<String, BackendError>> + Send + 'a>>;
}
```

- Non-zero exit → `Err(BackendError::Exit { code, message })`
- Spawn failure → `Err(BackendError::Spawn(...))`
- Stdin is NOT piped for non-pkexec commands (inherits parent process stdin, which is `/dev/null` in GUI context)
- Stdout + stderr captured concurrently and streamed line-by-line to the UI log panel

### 1.6 How Other Backends Call CommandExecutor

**Privileged** (APT, DNF, Pacman, Zypper): wrap command in `pkexec`:
```rust
runner.run("pkexec", &["apt", "upgrade", "-y"]).await
```

**Unprivileged** (Flatpak, Homebrew, Nix): call directly:
```rust
runner.run("flatpak", &["update", "-y"]).await
```

**`list_available()`** — all backends use `tokio::process::Command` directly (NOT through `runner`):
```rust
let out = tokio::process::Command::new("apt")
    .args(["list", "--upgradable"])
    .output().await?;
```

### 1.7 Detection Pattern

Each backend module has a standalone `is_available()` function using the `which` crate:
```rust
pub fn is_available() -> bool {
    which::which("fwupdmgr").is_ok()
}
```

`detect_backends()` in `mod.rs` calls each module's `is_available()` and pushes backends to a `Vec<Arc<dyn Backend>>`. Backends run in the order they are pushed (OS package manager first, then Nix, Flatpak, Homebrew).

### 1.8 `which` Crate

Already in `Cargo.toml` at version `"7"`. No new dependency needed.

### 1.9 `serde_json` Crate

Already in `Cargo.toml` at version `"1"`. Used in existing Nix backend for flake.lock parsing. No new dependency needed.

---

## 2. BackendKind::Fwupd Addition

### 2.1 Enum Change

Add `Fwupd` to `BackendKind` in `src/backends/mod.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendKind {
    Apt,
    Dnf,
    Pacman,
    Zypper,
    Flatpak,
    Homebrew,
    Nix,
    Fwupd,  // NEW
}
```

### 2.2 Display Impl Update

Add arm to the existing match in `Display for BackendKind`:

```rust
Self::Fwupd => write!(f, "Fwupd"),
```

### 2.3 Match Sites That Need Updating

| File | Match type | Action required |
|------|-----------|-----------------|
| `src/backends/mod.rs` | `Display` impl for `BackendKind` | Add `Self::Fwupd => write!(f, "Fwupd")` |

All other `BackendKind` usage is:
- Through trait methods (`backend.icon_name()`, `backend.display_name()`, etc.) — no match needed
- `Vec<BackendKind>` for skipped backends — no match needed
- `(BackendKind, UpdateRow)` tuples — no match needed

The `BackendKind` is serialized/deserialized via serde for `config.json` (`skipped_backends` field). Adding `Fwupd` is backward-compatible: existing config files lacking `"Fwupd"` will correctly deserialize with an empty skipped set for it.

---

## 3. FwupdBackend Struct Design

### 3.1 File Location

`src/backends/fwupd.rs` (new file)

### 3.2 Struct Definition

```rust
pub struct FwupdBackend;
```

No fields needed — `fwupdmgr` is stateless and detected/called at runtime.

### 3.3 Method Implementations

| Method | Return value | Notes |
|--------|-------------|-------|
| `kind()` | `BackendKind::Fwupd` | |
| `display_name()` | `"Firmware (fwupd)"` | |
| `description()` | `"Device firmware via LVFS"` | |
| `icon_name()` | `"firmware-manager-symbolic"` | Freedesktop icon for firmware management |
| `needs_root()` | `false` | fwupd daemon self-elevates via polkit D-Bus |
| `list_available()` | Parsed JSON from `fwupdmgr get-updates --json` | Exit code 2 → `Ok(vec![])` |
| `count_available()` | Trait default (delegates to `list_available`) | |
| `run_update()` | `runner.run("fwupdmgr", &["update"])` | No pkexec |
| `supports_cleanup()` | `false` (default) | fwupd has no cleanup concept |

---

## 4. Output Parsing Strategy

### 4.1 `fwupdmgr get-updates` Text Output Format

The text output uses a tree structure (example from fwupd issue #1698):

```
B450M DS3H
│
└─Unifying Receiver:
  │   Device ID:           cf3685ba249d3d98602047341d6f5a5556a6ac05
  │   Summary:             A miniaturised USB wireless receiver
  │   Current version:     RQR12.07_B0029
  │   Vendor:              Logitech, Inc. (USB:0x046D)
  │   Device Flags:        • Updatable
  │                        • Supported on remote server
  │
  ├─Unifying Receiver (RQR12) Device Update:
  │     New version:       RQR12.10_B0032
  │     Remote ID:         lvfs
  │     Summary:           Firmware for the Logitech Unifying Receiver (RQR12.xx)
```

**This text format is explicitly documented as unstable** (from the man page): "the terminal output between versions of fwupd is not guaranteed to be stable".

### 4.2 `fwupdmgr get-updates --json` Output Format

The `--json` flag produces stable machine-readable output. Structure:

```json
{
  "Devices": [
    {
      "DeviceId": "cf3685ba249d3d98602047341d6f5a5556a6ac05",
      "Name": "Unifying Receiver",
      "Version": "RQR12.07_B0029",
      "Vendor": "Logitech",
      "Releases": [
        {
          "AppstreamId": "...",
          "Version": "RQR12.10_B0032",
          "Summary": "Firmware for the Logitech Unifying Receiver",
          "Description": "..."
        }
      ]
    }
  ]
}
```

### 4.3 Parsing Implementation

```rust
/// Parse JSON output of `fwupdmgr get-updates --json`.
/// Returns a list of "<DeviceName> (<NewVersion>)" strings.
pub(crate) fn parse_fwupd_updates(json_text: &str) -> Vec<String> {
    let value: serde_json::Value = match serde_json::from_str(json_text) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("Failed to parse fwupd JSON: {e}");
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
                if let Some(first) = releases.first() {
                    let version = first
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
```

### 4.4 Exit Code Handling for `get-updates`

| Exit code | Meaning | Action |
|-----------|---------|--------|
| 0 | Updates available | Parse JSON, return list |
| 2 | No updates available | Return `Ok(vec![])` |
| 1 | Generic error | Return `Err(...)` |
| 3 | Resource not found | Return `Err(...)` (fwupd service not running) |

Exit code 2 is documented in the man page: "A return code of 2 is used for commands that have no actions but were successfully executed."

### 4.5 Update Output Parsing for `run_update`

`fwupdmgr update` output (representative sample):

```
Downloading ThinkPad BIOS 1.59...
Flashing ThinkPad System Firmware 1.59...
Successfully installed firmware
Pending updates require a reboot to complete
```

Count strategy — count lines indicating successful device updates:

```rust
pub(crate) fn count_fwupd_updated(output: &str) -> usize {
    output
        .lines()
        .filter(|l| {
            let t = l.trim();
            t.contains("Successfully installed") || t.starts_with("Updated ")
        })
        .count()
}
```

If no matching lines are found but the command exited 0, return 0 (some updates may require reboot and show no "installed" line until after reboot — that is acceptable).

---

## 5. Privilege Design

### 5.1 Architecture

`fwupdmgr` is a D-Bus client that communicates with `fwupd.service` (a system daemon). The daemon holds the privilege required to write firmware. When an operation requires elevated access, the daemon requests a polkit authorization via D-Bus, which surfaces as a graphical polkit dialog on the desktop. This happens **entirely within fwupd's own IPC layer** — no `pkexec` is involved.

### 5.2 Privilege Decision

| Aspect | Value | Reason |
|--------|-------|--------|
| `needs_root()` | `false` | No pkexec involved |
| `run_update` prefix | none (call `fwupdmgr` directly) | fwupd self-authorizes via polkit |
| Polkit dialog | Shown by the system | Triggered by fwupd daemon when needed |

### 5.3 Non-interactive Behaviour

`CommandRunner` does not pipe stdin to spawned processes (stdin inherits from the GUI app). In a GTK session, the app's stdin is `/dev/null`. `fwupdmgr` detects a non-TTY stdin and does not prompt for text confirmation. The polkit authentication dialog is a separate graphical D-Bus mechanism and still appears correctly.

### 5.4 `run_update` CommandExecutor Call

```rust
// No pkexec, no -y flag (not a documented fwupdmgr option)
runner.run("fwupdmgr", &["update"]).await
```

---

## 6. Detection and Registration

### 6.1 `is_available()` in `src/backends/fwupd.rs`

```rust
pub fn is_available() -> bool {
    which::which("fwupdmgr").is_ok()
}
```

### 6.2 Module Declaration in `src/backends/mod.rs`

Add at the top of `mod.rs` with the other module declarations:

```rust
pub mod fwupd;
```

### 6.3 `detect_backends()` Registration

Add after Homebrew in `detect_backends()`:

```rust
// fwupd — firmware updates via LVFS; unprivileged (polkit handled by daemon)
if fwupd::is_available() {
    backends.push(Arc::new(fwupd::FwupdBackend));
}
```

### 6.4 Complete Updated `detect_backends()` Function

```rust
pub fn detect_backends() -> Vec<Arc<dyn Backend>> {
    let mut backends: Vec<Arc<dyn Backend>> = Vec::new();

    // Detect OS package manager
    if let Some(os_backend) = os_package_manager::detect() {
        backends.push(os_backend);
    }

    // Nix — placed before Flatpak so that row order matches execution order
    if nix::is_available() {
        backends.push(Arc::new(nix::NixBackend));
    }

    // Flatpak
    if flatpak::is_available() || flatpak::is_running_in_flatpak() {
        backends.push(Arc::new(flatpak::FlatpakBackend));
    }

    // Homebrew
    if homebrew::is_available() {
        backends.push(Arc::new(homebrew::HomebrewBackend));
    }

    // fwupd — firmware updates via LVFS
    if fwupd::is_available() {
        backends.push(Arc::new(fwupd::FwupdBackend));
    }

    for b in &backends {
        info!("Backend detected: {}", b.display_name());
    }

    backends
}
```

---

## 7. Implementation Steps (Ordered, File-by-File)

### Step 1 — `src/backends/mod.rs`

**Change 1:** Add module declaration at the top of the file with the other module declarations:
```rust
pub mod fwupd;
```

**Change 2:** Add `Fwupd` variant to `BackendKind` enum (after `Nix`):
```rust
pub enum BackendKind {
    Apt,
    Dnf,
    Pacman,
    Zypper,
    Flatpak,
    Homebrew,
    Nix,
    Fwupd,  // NEW
}
```

**Change 3:** Add arm to `Display for BackendKind` match (after `Self::Nix`):
```rust
Self::Fwupd => write!(f, "Fwupd"),
```

**Change 4:** Add fwupd detection to `detect_backends()` (after the Homebrew block):
```rust
// fwupd — firmware updates via LVFS; unprivileged (polkit handled by daemon)
if fwupd::is_available() {
    backends.push(Arc::new(fwupd::FwupdBackend));
}
```

### Step 2 — `src/backends/fwupd.rs` (new file)

Create the complete implementation (see Section 8).

---

## 8. New File: `src/backends/fwupd.rs`

```rust
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
                // fwupd service not running, or other error — log and return empty
                // rather than propagating an error that would alarm the user.
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

    #[test]
    fn test_parse_fwupd_updates_single_device() {
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
                }
            ]
        }"#;
        let updates = parse_fwupd_updates(json);
        assert_eq!(updates, vec!["Unifying Receiver (RQR12.10_B0032)"]);
    }

    #[test]
    fn test_parse_fwupd_updates_no_devices() {
        let json = r#"{"Devices": []}"#;
        let updates = parse_fwupd_updates(json);
        assert!(updates.is_empty());
    }

    #[test]
    fn test_parse_fwupd_updates_no_releases() {
        // Device with no Releases array should be skipped
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

    #[test]
    fn test_parse_fwupd_updates_multiple_devices() {
        let json = r#"{
            "Devices": [
                {
                    "Name": "ThinkPad System Firmware",
                    "Releases": [{ "Version": "1.56" }]
                },
                {
                    "Name": "NVMe Drive",
                    "Releases": [{ "Version": "2.5" }]
                }
            ]
        }"#;
        let updates = parse_fwupd_updates(json);
        assert_eq!(updates.len(), 2);
        assert!(updates.contains(&"ThinkPad System Firmware (1.56)".to_string()));
        assert!(updates.contains(&"NVMe Drive (2.5)".to_string()));
    }

    #[test]
    fn test_parse_fwupd_updates_invalid_json() {
        let updates = parse_fwupd_updates("not valid json");
        assert!(updates.is_empty());
    }

    #[test]
    fn test_count_fwupd_updated_success_lines() {
        let output = "Downloading firmware...\nSuccessfully installed firmware\nPending reboot";
        assert_eq!(count_fwupd_updated(output), 1);
    }

    #[test]
    fn test_count_fwupd_updated_staged_only() {
        // Updates staged for reboot produce no "Successfully installed" line
        let output = "Flashing ThinkPad BIOS...\nPending updates require a reboot";
        assert_eq!(count_fwupd_updated(output), 0);
    }

    #[test]
    fn test_count_fwupd_updated_empty() {
        assert_eq!(count_fwupd_updated(""), 0);
    }
}
```

---

## 9. Risks & Mitigations

| Risk | Probability | Mitigation |
|------|-------------|-----------|
| `fwupdmgr` not installed | Medium | `is_available()` returns `false` → backend not registered. Handled by `which` check. |
| `fwupd.service` not running / failed to start | Low-Medium | `fwupdmgr get-updates` returns non-zero exit → `list_available` returns `Err(...)` which is treated as "unable to check" in the UI. `run_update` returns `UpdateResult::Error`. |
| JSON format differs across fwupd versions | Low | `parse_fwupd_updates` returns `Ok(vec![])` on any parse failure (logs warning). The `--json` flag has been stable since fwupd 1.x; all major distros ship ≥1.5. |
| `fwupdmgr update` prompts for confirmation | Very Low | No TTY stdin in a GTK app → fwupdmgr detects non-interactive mode and does not prompt. Polkit dialog appears graphically via D-Bus. |
| Staged updates (require reboot) reported as count 0 | Accepted | `UpdateResult::Success { updated_count: 0 }` is correct — the update IS staged. UI shows "Up to date" after reboot. This is acceptable behaviour. |
| Flatpak sandbox | Medium | Inside the Flatpak sandbox, `fwupdmgr` is not on the sandbox PATH. `is_available()` returns `false`, backend is not registered. Firmware updates should be handled by the host system separately. |
| Network unavailable / LVFS unreachable | Low | `fwupdmgr get-updates` uses cached metadata from last `refresh`. Metadata refresh (`fwupd-refresh.timer`) runs separately by systemd. No network call needed at check time. |
| `firmware-manager-symbolic` icon not present | Low | GTK falls back to a generic icon if not found. No crash. |

---

## 10. Summary of Findings

### Backend Pattern
- `FwupdBackend` follows the exact same pattern as `FlatpakBackend` and `HomebrewBackend` (unprivileged, no `pkexec`)
- `list_available()` uses `tokio::process::Command` directly (consistent with all other backends)
- `run_update()` goes through `runner.run()` (consistent with all other backends)
- No new dependencies required (`which` and `serde_json` already in `Cargo.toml`)

### fwupd Command Behaviour
- `fwupdmgr get-updates --json`: exit 0 = updates available, exit 2 = no updates, exit 1/3 = error
- `fwupdmgr update`: no `-y` flag; polkit dialog triggered by daemon via D-Bus; safe to call without pkexec
- Binary name: always `fwupdmgr` (never `fwupd-client`)

### Privilege Model
- `needs_root()` = `false`
- No `pkexec` in any call
- fwupd daemon handles its own polkit authorization

### Files Modified/Created
| File | Action |
|------|--------|
| `src/backends/mod.rs` | Modified — add `pub mod fwupd`, `BackendKind::Fwupd`, Display arm, detection call |
| `src/backends/fwupd.rs` | Created — full backend implementation |

---

**Spec file path:** `c:\Projects\Up\.github\docs\subagent_docs\fwupd_backend_spec.md`
