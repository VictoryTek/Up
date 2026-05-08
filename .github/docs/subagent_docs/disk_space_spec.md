# Disk-Space Pre-Check — Feature Specification

> Status: Draft  
> Author: Research Subagent  
> Date: May 8, 2026  
> Target: Up v1.0.4  

---

## 1. Current State

### Update Check Flow

`src/ui/window.rs` contains a `run_checks` closure (defined inside `build_update_page`) that:

1. Iterates over all detected `Arc<dyn Backend>` instances.
2. For each backend, spawns a background task (via `super::spawn_background_async`) that calls:
   - `backend.count_available().await` → `Result<usize, String>`
   - `backend.list_available().await` → `Result<Vec<String>, String>`
3. Both results are sent back through a `async_channel::bounded(1)` channel.
4. The GTK main thread receives them and calls:
   - `row.set_status_available(count)` — updates the trailing `status_label`
   - `row.set_packages(packages)` — populates the expandable child rows
5. After the last backend finishes, the "Update All" button becomes sensitive if `total_available > 0`.

The channel payload type is currently:
```rust
type CheckPayload = (Result<usize, String>, Result<Vec<String>, String>);
```

### Backend Trait (`src/backends/mod.rs`)

The `Backend` trait currently has:

| Method | Default | Purpose |
|--------|---------|---------|
| `kind()` | required | `BackendKind` enum |
| `display_name()` | required | UI label |
| `description()` | required | UI subtitle |
| `icon_name()` | required | GTK icon name |
| `run_update()` | required | Apply updates |
| `needs_root()` | `false` | Whether pkexec is needed |
| `count_available()` | delegates to `list_available` | Count pending updates |
| `list_available()` | returns `Ok(vec![])` | List package names |
| `supports_cleanup()` | `false` | Whether cleanup is supported |
| `run_cleanup()` | no-op | Run maintenance |

### What Size Data Currently Exists

| Backend | Size Data Available? | Notes |
|---------|---------------------|-------|
| APT | Not fetched | `apt-get -s upgrade` would provide it |
| DNF | Not fetched | `dnf upgrade --assumeno` would provide it |
| Pacman | Not fetchable | No reliable dry-run without network calls |
| Zypper | Not fetched | `zypper update --dry-run` would provide it |
| Flatpak | Not fetched | `flatpak remote-ls --columns=download-size` would provide it |
| Homebrew | Not fetchable | No dry-run/size flag |
| Nix | Not fetchable | Complex, version-dependent |
| fwupd | Present in JSON, not parsed | `Releases[].Size` in `fwupdmgr get-updates --json` |

The `parse_fwupd_updates` function in `src/backends/fwupd.rs` already parses device names and versions from the JSON response but discards the `Releases[].Size` field.

### UI Components

`UpdateRow` (`src/ui/update_row.rs`) wraps `adw::ExpanderRow`:
- **Title**: `backend.display_name()` (e.g., "APT")
- **Subtitle**: `backend.description()` (e.g., "Debian / Ubuntu packages") — set in builder, never changed at runtime
- **Trailing suffix widgets**: spinner, skip checkbox, retry button, `status_label`
- **`status_label`**: trailing `gtk::Label` with `max_width_chars(30)`, showing "Ready", "Checking...", "N packages available", etc.

The `adw::ExpanderRow` is an `adw::PreferencesRow` (which is an `adw::ActionRow` subclass), so `set_subtitle()` can be called at runtime.

### Existing Warning Dialog Pattern

The `update_button.connect_clicked` handler in `window.rs` already implements a chain of pre-flight checks using `adw::AlertDialog` and `bypass_*: Rc<Cell<bool>>` flags:

1. **Metered connection check** (`bypass_metered`)
2. **Battery check** (`bypass_battery`) 
3. **Snapshot check** (`bypass_snapshot`)

Each check:
- Shows an `adw::AlertDialog` with "Cancel" and "Update Anyway" responses
- On "Update Anyway": sets the bypass flag, calls `button.emit_clicked()`, resets the flag
- On "Cancel": returns without proceeding

The disk-space check will slot into this chain as step **2.5** (between battery and snapshot).

---

## 2. Problem Statement

Users on systems with limited disk space risk failed or corrupted package installations when updates run out of space midway through an operation. APT, DNF, and Zypper all produce broken states when a transaction runs out of space; manual recovery (running `dpkg --configure -a`, `dnf clean all`, `rpm --rebuilddb`) is not user-friendly.

Up has no mechanism to:
- Estimate how much disk space an update will consume before applying it
- Warn the user if available free space is insufficient
- Surface per-backend size information to help users make informed decisions

This is the last "medium effort" unchecked item in the Codebase Analysis backlog:
> **Disk-space pre-check** — surface transaction size from APT/DNF/Flatpak before applying

---

## 3. Design Decisions

### 3.1 Trait Extension vs. In-Band Return

**Option A — New trait method `estimate_size()`**: A new optional method on `Backend` that returns `Option<u64>`. Backends that cannot estimate return `None` (the default). Size data is fetched in the same background task as `list_available()`.

**Option B — Enrich `list_available()` return type**: Change the return type to carry size alongside package names.

**Decision: Option A** — a separate `estimate_size()` method with default `None`. Reasons:
- Non-breaking: all existing backends automatically get `None` behaviour.
- Semantically clean: size estimation is a distinct capability from package enumeration.
- Some backends (Flatpak) use a different command to get sizes vs. listing.
- `count_available()` → `list_available()` → `estimate_size()` mirrors the existing delegation pattern.

### 3.2 Available Space Detection

**Options considered**:
- `std::fs::metadata` on "/" — does not give free space.
- `nix` crate `statvfs` syscall — requires a new Cargo dependency (prohibited by constraint).
- `fs2` crate — requires a new Cargo dependency (prohibited by constraint).
- `df -k /` subprocess with `LANG=C` — uses no new dependencies; reliable on all Linux distros.

**Decision: `df -k /`** — parsed with `LANG=C LC_ALL=C` to ensure English-locale output. Spawned as a background async task. The result is cached in `Rc<Cell<Option<u64>>>` during the check cycle.

### 3.3 UI Placement

**Option A — Status label** (`status_label` in the trailing suffix): "12 packages — 234 MB" — constrained to `max_width_chars(30)`.

**Option B — Row subtitle** (the `adw::ExpanderRow` subtitle below the title): "Debian / Ubuntu packages — 234 MB needed" — naturally wider, not constrained.

**Option C — `adw::Banner` at the top of the update list** — prominent, but requires a new widget that may conflict with existing metered/restart banners.

**Decision: Option B (subtitle update) + Option C (warning dialog)** — update the subtitle dynamically to show the size, and show an `adw::AlertDialog` warning when total required space exceeds available. This matches the existing dialog chain pattern perfectly.

### 3.4 Which Backends Implement `estimate_size()`

| Backend | Implements? | Method | Reason for Skip |
|---------|-------------|--------|-----------------|
| APT | ✅ Yes | `apt-get -s upgrade` | Dry-run, exits 0, clear size output |
| DNF | ✅ Yes | `dnf upgrade --assumeno` | Non-zero exit OK; prints transaction summary |
| Zypper | ✅ Yes | `zypper update --dry-run` | Prints "After the operation" line |
| Flatpak | ✅ Yes | `flatpak remote-ls --columns=download-size` | Per-app download sizes available |
| fwupd | ✅ Yes | `fwupdmgr get-updates --json` (re-parsed) | `Releases[].Size` is in bytes |
| Pacman | ❌ No | — | No standard dry-run; `pacman -Sup` requires network for size |
| Homebrew | ❌ No | — | No `--dry-run`; bottle sizes require per-formula network lookups |
| Nix | ❌ No | — | `nix --dry-run` output varies significantly between versions; unreliable parsing |

### 3.5 Warning Threshold

Warn when: `available_space_bytes < required_bytes * 11 / 10`

This is equivalent to "required × 1.1 > available", i.e., there is less than 10% headroom. Integer arithmetic is used to avoid floating-point.

### 3.6 New Module: `src/disk.rs`

Following the project's pattern (`battery.rs`, `reboot.rs`, `snapshot.rs`), size-related utilities live in a dedicated module:
- `detect_available_space() -> Option<u64>` (async)
- `parse_df_available(text: &str) -> Option<u64>` (testable pure function)
- `format_bytes(bytes: u64) -> String` (shared formatter used by update_row and warning dialog)
- `parse_size_value(n: f64, unit: &str) -> u64` (shared unit parser used by APT/DNF/Zypper/Flatpak)

`src/main.rs` will need `mod disk;` added.

---

## 4. Architecture

### 4.1 New Trait Method

**File**: `src/backends/mod.rs`

Add to the `Backend` trait immediately after `list_available`:

```rust
/// Estimate the total additional disk space (in bytes) this backend's pending
/// updates will require after installation.  Returns `None` when estimation is
/// not supported or the command fails.
///
/// The default implementation returns `None`.  Backends that can produce a
/// reliable estimate (APT, DNF, Zypper, Flatpak, fwupd) override this method.
///
/// This is called after `list_available()` on the background thread; failures
/// are silent (treated as `None`).
fn estimate_size(&self) -> Pin<Box<dyn Future<Output = Option<u64>> + Send + '_>> {
    Box::pin(async { None })
}
```

### 4.2 New Utility Module

**File**: `src/disk.rs` (new file)

```rust
// src/disk.rs

/// Detect the available disk space on the root filesystem in bytes.
/// Spawns `df -k /` with `LANG=C` and parses the "Available" column.
/// Returns `None` on spawn failure or parse error.
pub async fn detect_available_space() -> Option<u64> { ... }

/// Parse the output of `df -k /` and return available bytes.
/// The "Available" column is in 1-KiB blocks; multiply by 1024.
/// Handles long filesystem names that cause df to wrap across two lines.
pub(crate) fn parse_df_available(text: &str) -> Option<u64> { ... }

/// Format a byte count as a human-readable string.
/// - < 1 MiB  → "N KB"
/// - < 1 GiB  → "N MB"
/// - ≥ 1 GiB  → "N.N GB"
pub fn format_bytes(bytes: u64) -> String { ... }

/// Convert a numeric value + unit string to bytes.
/// Recognised units (case-insensitive): k/kb/kib → ×1024,
/// m/mb/mib → ×1048576, g/gb/gib → ×1073741824.
/// Unknown units are treated as bytes.
pub(crate) fn parse_size_value(n: f64, unit: &str) -> u64 { ... }
```

### 4.3 Per-Backend `estimate_size()` Implementations

#### 4.3.1 APT (`src/backends/os_package_manager.rs`)

```rust
impl Backend for AptBackend {
    // ... existing methods ...

    fn estimate_size(&self) -> Pin<Box<dyn Future<Output = Option<u64>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("apt-get")
                .args(["-s", "upgrade"])
                .env("LANG", "C")
                .env("LC_ALL", "C")
                .env("DEBIAN_FRONTEND", "noninteractive")
                .output()
                .await
                .ok()?;
            if !out.status.success() {
                return None;
            }
            let text = String::from_utf8_lossy(&out.stdout);
            crate::disk::parse_apt_size(&text)
        })
    }
}
```

New parser (added to `src/disk.rs` or at the bottom of `os_package_manager.rs`):

```rust
// In src/disk.rs (pub(crate)) — called from os_package_manager.rs
pub(crate) fn parse_apt_size(output: &str) -> Option<u64> {
    // Matches: "After this operation, 234 MB of additional disk space will be used."
    // Matches: "After this operation, 234 kB disk space will be freed."
    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("After this operation,") && t.contains("disk space") {
            let parts: Vec<&str> = t.split_whitespace().collect();
            // Tokens: ["After","this","operation,","234","MB","of","additional",...,"used."]
            //                                         ^3    ^4
            if parts.len() >= 5 {
                if let Ok(n) = parts[3].parse::<f64>() {
                    let unit = parts[4];
                    if t.contains("freed") {
                        return Some(0);
                    }
                    return Some(parse_size_value(n, unit));
                }
            }
        }
    }
    None
}
```

#### 4.3.2 DNF (`src/backends/os_package_manager.rs`)

```rust
impl Backend for DnfBackend {
    // ... existing methods ...

    fn estimate_size(&self) -> Pin<Box<dyn Future<Output = Option<u64>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("dnf")
                .args(["upgrade", "--assumeno"])
                .env("LANG", "C")
                .env("LC_ALL", "C")
                .output()
                .await
                .ok()?;
            // DNF exits non-zero (1 or 5) when packages exist; still prints summary.
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let combined = format!("{stdout}\n{stderr}");
            crate::disk::parse_dnf_install_size(&combined)
        })
    }
}
```

```rust
// In src/disk.rs
pub(crate) fn parse_dnf_install_size(output: &str) -> Option<u64> {
    // Priority 1: "Disk usage after transaction: +141 M"  (DNF5)
    // Priority 2: "Total installed size: 141 M"            (DNF4)
    // Fallback:   "Total download size: 52 M"              (any)
    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("Disk usage after transaction:")
            || t.starts_with("Total installed size:")
        {
            if let Some(v) = parse_dnf_size_line(t) {
                return Some(v);
            }
        }
    }
    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("Total download size:") {
            if let Some(v) = parse_dnf_size_line(t) {
                return Some(v);
            }
        }
    }
    None
}

fn parse_dnf_size_line(line: &str) -> Option<u64> {
    // "Total download size: 52 M"  →  tokens after ":" are "52" "M"
    // "Disk usage after transaction: +141 M"  →  token "+141" needs trimming
    for token in line.split_whitespace() {
        let n_str = token.trim_start_matches('+').trim_start_matches('-');
        if let Ok(n) = n_str.parse::<f64>() {
            // Unit is the next whitespace token — grab it from the original line
            if let Some(unit) = line
                .split_whitespace()
                .skip_while(|t| {
                    t.trim_start_matches('+')
                     .trim_start_matches('-')
                     .parse::<f64>()
                     .is_err()
                })
                .nth(1)
            {
                return Some(parse_size_value(n, unit));
            }
        }
    }
    None
}
```

#### 4.3.3 Zypper (`src/backends/os_package_manager.rs`)

```rust
impl Backend for ZypperBackend {
    // ... existing methods ...

    fn estimate_size(&self) -> Pin<Box<dyn Future<Output = Option<u64>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("zypper")
                .args(["--non-interactive", "--no-color", "update", "--dry-run"])
                .env("LANG", "C")
                .env("LC_ALL", "C")
                .output()
                .await
                .ok()?;
            let text = String::from_utf8_lossy(&out.stdout);
            crate::disk::parse_zypper_size(&text)
        })
    }
}
```

```rust
// In src/disk.rs
pub(crate) fn parse_zypper_size(output: &str) -> Option<u64> {
    // "After the operation, additional 141 MiB will be used."
    // "After the operation, 141 MiB will be freed."
    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("After the operation,") {
            let parts: Vec<&str> = t.split_whitespace().collect();
            // parts: ["After","the","operation,","additional","141","MiB","will","be","used."]
            //   or:  ["After","the","operation,","141","MiB","will","be","freed."]
            let (num_idx, unit_idx) = if parts.get(3) == Some(&"additional") {
                (4, 5)
            } else {
                (3, 4)
            };
            if let (Some(&num_str), Some(&unit)) =
                (parts.get(num_idx), parts.get(unit_idx))
            {
                if let Ok(n) = num_str.parse::<f64>() {
                    if t.contains("freed") {
                        return Some(0);
                    }
                    return Some(parse_size_value(n, unit));
                }
            }
        }
    }
    None
}
```

#### 4.3.4 Flatpak (`src/backends/flatpak.rs`)

```rust
impl Backend for FlatpakBackend {
    // ... existing methods ...

    fn estimate_size(&self) -> Pin<Box<dyn Future<Output = Option<u64>> + Send + '_>> {
        Box::pin(async move {
            let (cmd, args) = build_flatpak_cmd(&[
                "remote-ls", "--updates", "--user",
                "--columns=download-size",
            ]);
            let out = tokio::process::Command::new(&cmd)
                .args(&args)
                .output()
                .await
                .ok()?;
            if !out.status.success() {
                return None;
            }
            let text = String::from_utf8_lossy(&out.stdout);
            let total = crate::disk::parse_flatpak_download_sizes(&text);
            if total == 0 { None } else { Some(total) }
        })
    }
}
```

```rust
// In src/disk.rs
/// Parse output from `flatpak remote-ls --updates --columns=download-size`.
/// Each line is a human-readable size like "12.3 MB" or "1.2 kB".
/// Lines that are not parseable (e.g., column header "Download", "-") are skipped.
pub(crate) fn parse_flatpak_download_sizes(output: &str) -> u64 {
    output
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.trim().split_whitespace().collect();
            if parts.len() >= 2 {
                if let Ok(n) = parts[0].parse::<f64>() {
                    return Some(parse_size_value(n, parts[1]));
                }
            }
            None
        })
        .sum()
}
```

#### 4.3.5 fwupd (`src/backends/fwupd.rs`)

```rust
impl Backend for FwupdBackend {
    // ... existing methods ...

    fn estimate_size(&self) -> Pin<Box<dyn Future<Output = Option<u64>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("fwupdmgr")
                .args(["get-updates", "--json"])
                .output()
                .await
                .ok()?;

            let code = out.status.code().unwrap_or(-1);
            // Exit code 2 = no updates.
            if code == 2 {
                return Some(0);
            }
            if !out.status.success() {
                return None;
            }

            let text = String::from_utf8_lossy(&out.stdout);
            let total = crate::disk::parse_fwupd_size(&text);
            if total == 0 { None } else { Some(total) }
        })
    }
}
```

```rust
// In src/disk.rs
/// Parse the total download size in bytes from `fwupdmgr get-updates --json`.
/// Sums `Devices[].Releases[0].Size` (bytes) for all devices with pending updates.
pub(crate) fn parse_fwupd_size(json_text: &str) -> u64 {
    let value: serde_json::Value = match serde_json::from_str(json_text) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    let mut total: u64 = 0;
    if let Some(devices) = value.get("Devices").and_then(|d| d.as_array()) {
        for device in devices {
            if let Some(releases) = device.get("Releases").and_then(|r| r.as_array()) {
                if let Some(first) = releases.first() {
                    if let Some(size) = first.get("Size").and_then(|s| s.as_u64()) {
                        total += size;
                    }
                }
            }
        }
    }
    total
}
```

### 4.4 `UpdateRow` Changes (`src/ui/update_row.rs`)

Add two fields to `UpdateRow`:

```rust
pub struct UpdateRow {
    // ... existing fields ...
    /// Base description string (backend.description()); stored so the subtitle
    /// can be reconstructed with or without size information.
    base_description: String,
    /// Last estimated required disk space in bytes; None if not yet fetched or unsupported.
    last_size_bytes: Rc<Cell<Option<u64>>>,
}
```

In `UpdateRow::new()`:
- Capture `backend.description().to_string()` into `base_description`.
- Initialise `last_size_bytes: Rc::new(Cell::new(None))`.
- Include both in the `Self { ... }` constructor return.

New public method:

```rust
/// Update the subtitle to include the estimated required disk space.
/// If `size_bytes` is `None` or 0, reverts subtitle to the base description.
/// Call this on the GTK main thread after `estimate_size()` completes.
pub fn set_download_size(&self, size_bytes: Option<u64>) {
    *self.last_size_bytes.borrow_mut() = size_bytes;  // wait — use Cell, not RefCell
    self.last_size_bytes.set(size_bytes);
    let subtitle = match size_bytes {
        Some(bytes) if bytes > 0 => {
            format!(
                "{} — {} needed",
                self.base_description,
                crate::disk::format_bytes(bytes)
            )
        }
        _ => self.base_description.clone(),
    };
    self.row.set_subtitle(&subtitle);
}

/// Returns the last estimated required disk space in bytes.
/// `None` if estimate is not available or not yet fetched.
pub fn last_size_bytes(&self) -> Option<u64> {
    self.last_size_bytes.get()
}
```

Note: `last_size_bytes` uses `Rc<Cell<Option<u64>>>` (same pattern as `last_available: Rc<Cell<Option<usize>>>`).

On `set_status_checking()`, reset the size so stale data isn't shown during re-check:

```rust
pub fn set_status_checking(&self) {
    // ... existing code ...
    self.last_size_bytes.set(None);
    self.row.set_subtitle(&self.base_description);  // reset subtitle
}
```

### 4.5 `window.rs` Integration (`src/ui/window.rs`)

#### 4.5.1 New State in `build_update_page`

```rust
// After existing Rc declarations:
let available_space_bytes: Rc<Cell<Option<u64>>> = Rc::new(Cell::new(None));
let bypass_disk_space: Rc<Cell<bool>> = Rc::new(Cell::new(false));
```

#### 4.5.2 Extend `run_checks` Channel Type

Change the channel payload type from:
```rust
type CheckPayload = (Result<usize, String>, Result<Vec<String>, String>);
```
to:
```rust
type CheckPayload = (Result<usize, String>, Result<Vec<String>, String>, Option<u64>);
```

Extend the background task:
```rust
super::spawn_background_async(move || async move {
    let count = backend_clone.count_available().await;
    let list  = backend_clone.list_available().await;
    let size  = backend_clone.estimate_size().await;   // NEW
    let _ = tx.send((count, list, size)).await;
});
```

Extend the GTK-thread receive handler:
```rust
if let Ok((count_result, list_result, size_result)) = rx.recv().await {
    // ... existing epoch check ...
    // ... existing count / list handling ...

    // NEW: apply size to the row subtitle
    row.set_download_size(size_result);
    // ... existing pending_checks countdown ...

    // When all checks are done, also detect available space
    if remaining == 0 {
        // ... existing update_button enable logic ...

        // Detect available space (non-blocking; failure is graceful)
        let avail_space_cell = available_space_bytes.clone();
        super::spawn_background_async(move || async move {
            let space = crate::disk::detect_available_space().await;
            // Send result back to GTK thread
            let (sp_tx, sp_rx) = async_channel::bounded::<Option<u64>>(1);
            let _ = sp_tx.send(space).await;
            drop(sp_tx);
            // The receiver is handled below via glib::spawn_future_local
            // (Note: implementation uses a shared channel pattern matching window.rs conventions)
        });
        // Preferred implementation: inline the detect + store:
        let avail_cell = available_space_bytes.clone();
        glib::spawn_future_local(async move {
            let space = crate::disk::detect_available_space().await;
            avail_cell.set(space);
        });
    }
}
```

**Simpler approach** (preferred): since `glib::spawn_future_local` can run async code on the GTK main thread and `detect_available_space` uses `tokio::process::Command` (which requires a Tokio runtime context), we must spawn it via `super::spawn_background_async`. Use the existing pattern from the backend checks:

```rust
// After remaining == 0, inside the glib::spawn_future_local block:
if remaining == 0 {
    // ... existing sensitivity + status_label logic ...

    // Kick off available-space detection asynchronously.
    let (sp_tx, sp_rx) = async_channel::bounded::<Option<u64>>(1);
    super::spawn_background_async(move || async move {
        let _ = sp_tx.send(crate::disk::detect_available_space().await).await;
    });
    let avail_cell = available_space_bytes.clone();
    glib::spawn_future_local(async move {
        if let Ok(space) = sp_rx.recv().await {
            avail_cell.set(space);
        }
    });
}
```

#### 4.5.3 Warning Dialog in `update_button.connect_clicked`

The warning is inserted **after the battery check and before the snapshot check**. Add `available_space_bytes` and `bypass_disk_space` to the closure's capture list via `#[strong]`.

```rust
// After bypass_battery block, before bypass_snapshot block:
if !bypass_disk_space.get() {
    // Sum required bytes for all non-skipped backends that have size data.
    let required_bytes: u64 = {
        let borrowed = rows.borrow();
        borrowed
            .iter()
            .filter(|(_, r)| !r.is_skipped())
            .filter_map(|(_, r)| r.last_size_bytes())
            .sum()
    };
    if required_bytes > 0 {
        if let Some(avail) = available_space_bytes.get() {
            // Threshold: warn if available < required * 1.1
            // Integer form: required * 11 > available * 10
            if required_bytes.saturating_mul(11) > avail.saturating_mul(10) {
                let msg = format!(
                    gettext(
                        "This update requires {} but only {} of disk space is available. \
                         The update may fail or leave packages in a broken state.\n\nProceed anyway?"
                    ),
                    crate::disk::format_bytes(required_bytes),
                    crate::disk::format_bytes(avail),
                );
                let dialog = adw::AlertDialog::new(
                    Some(&gettext("Not Enough Disk Space")),
                    Some(&msg),
                );
                dialog.add_response("cancel", &gettext("Cancel"));
                dialog.add_response("update", &gettext("Update Anyway"));
                dialog.set_response_appearance(
                    "update",
                    adw::ResponseAppearance::Destructive,
                );
                dialog.set_default_response(Some("cancel"));
                dialog.set_close_response("cancel");
                dialog.connect_response(
                    None,
                    glib::clone!(
                        #[weak]
                        button,
                        #[strong]
                        bypass_disk_space,
                        move |_, response| {
                            if response == "update" {
                                bypass_disk_space.set(true);
                                button.emit_clicked();
                                bypass_disk_space.set(false);
                            }
                        }
                    ),
                );
                dialog.present(Some(button));
                return;
            }
        }
    }
}
```

---

## 5. Exact Command Strings

All subprocess invocations use `LANG=C LC_ALL=C` to suppress locale-dependent output.

### APT

```
Command:  apt-get -s upgrade
Env:      LANG=C, LC_ALL=C, DEBIAN_FRONTEND=noninteractive
Exit:     0 (even when updates exist — this is a simulation)
Relevant output line examples:
  "After this operation, 234 MB of additional disk space will be used."
  "After this operation, 1,234 kB of additional disk space will be used."
  "After this operation, 1.23 GB of additional disk space will be used."
  "After this operation, 4,096 kB disk space will be freed."
Parse:    Find line starting with "After this operation," containing "disk space".
          Split on whitespace: token[3] = number, token[4] = unit.
          If line contains "freed", return 0.
```

### DNF

```
Command:  dnf upgrade --assumeno
Env:      LANG=C, LC_ALL=C
Exit:     Non-zero (1 or 5) when packages would be upgraded — acceptable; stdout still has summary.
          Exit 0 when nothing to upgrade.
Relevant output line examples (DNF4):
  "Total download size: 52 M"
  "Total installed size: 141 M"
Relevant output line examples (DNF5):
  "Total download size: 52 M"
  "Disk usage after transaction: +141 M"
Parse priority:
  1. Line starting with "Disk usage after transaction:" → installed size delta (DNF5)
  2. Line starting with "Total installed size:"         → installed size (DNF4)
  3. Line starting with "Total download size:"          → download size (fallback)
  Strip leading "+" or "-" from numeric tokens.
  Unit may be "k","M","G" (treat "k" as kB, "M" as MB, "G" as GB).
```

### Zypper

```
Command:  zypper --non-interactive --no-color update --dry-run
Env:      LANG=C, LC_ALL=C
Exit:     0 on success (even with pending updates)
Relevant output line examples:
  "After the operation, additional 141 MiB will be used."
  "After the operation, 141 MiB will be freed."
  "After the operation, additional 1.2 GiB will be used."
Parse:    Find line starting with "After the operation,".
          Check if token[3] is "additional" (shifts indices by 1).
          Extract numeric + unit. If "freed", return 0.
```

### Flatpak

```
Command:  flatpak remote-ls --updates --user --columns=download-size
          (Inside Flatpak sandbox: flatpak-spawn --host flatpak remote-ls ...)
Env:      (no LANG override needed; --columns output is not locale-sensitive for numeric values)
Exit:     0
Relevant output line examples:
  "12.3 MB"
  "1.2 kB"
  "947.2 kB"
  "-"            (unknown / not applicable → skip)
  "Download"     (column header → skip — parse_f64 will fail, which handles this)
Parse:    For each line, split on whitespace.
          If token[0] parses as f64, treat token[1] as unit and accumulate.
          Sum all parseable lines.
```

### fwupd

```
Command:  fwupdmgr get-updates --json
Exit:     0 when updates exist, 2 when no updates (treat as 0 bytes)
Relevant JSON path:  $.Devices[*].Releases[0].Size  (integer, bytes)
Parse:    Deserialise JSON, iterate Devices array,
          for each device take the first element of Releases[],
          read the "Size" key as u64.
          Sum all sizes.
```

### Available Space Detection

```
Command:  df -k /
Env:      LANG=C, LC_ALL=C
Exit:     0
Output example:
  Filesystem     1K-blocks      Used Available Use% Mounted on
  /dev/sda1       50000000  25000000  23000000  53% /
Long-path wrapping example:
  Filesystem
                  1K-blocks      Used Available Use% Mounted on
  /dev/mapper/ubuntu--vg-ubuntu--lv
                   50000000  25000000  23000000  53% /
Parse:    Skip header line.
          Re-join any whitespace-leading continuation lines with the previous line.
          Find the line whose last whitespace token is "/".
          Extract token[3] (0-indexed from that line's fields).
          Multiply by 1024 to convert KiB → bytes.
```

---

## 6. Size Parsing Logic

### `parse_size_value(n: f64, unit: &str) -> u64`

```
Canonical unit mappings (case-insensitive matching on unit.to_lowercase()):
  "b"                   → n as u64
  "k" | "kb" | "kib"   → (n * 1_024.0) as u64
  "m" | "mb" | "mib"   → (n * 1_048_576.0) as u64
  "g" | "gb" | "gib"   → (n * 1_073_741_824.0) as u64
  _   (unknown)         → n as u64   (treat as bytes — conservative)
```

### `format_bytes(bytes: u64) -> String`

```
bytes < 1_048_576                       → "{} KB"  where N = (bytes + 511) / 1024
bytes < 1_073_741_824                   → "{} MB"  where N = (bytes + 524_288) / 1_048_576
bytes ≥ 1_073_741_824                   → "{:.1} GB" where N = bytes / 1_073_741_824.0
```

Rounding: KB and MB use integer division with +½ unit to round to nearest. GB uses one decimal place via `f64` formatting.

Examples:
- 512 → "1 KB"
- 1_023 → "1 KB"
- 1_024 → "1 KB"
- 1_048_575 → "1024 KB"  (just below 1 MB threshold)  ← actually 1 MB = 1_048_576
- 1_048_576 → "1 MB"
- 234_000_000 → "223 MB"
- 1_500_000_000 → "1.4 GB"

### `parse_df_available(text: &str) -> Option<u64>`

```
Algorithm:
1. Split text into lines.
2. Skip the first line (header: "Filesystem  1K-blocks  Used  Available  Use%  Mounted on").
3. Iterate lines; if a line starts with whitespace, it is a continuation —
   prepend it (trimmed) to the previous line.
4. For each reconstructed line, split on whitespace:
   - If last token == "/" and there are at least 6 tokens:
     - Parse token[3] as u64 (Available, in 1K blocks)
     - Return token[3] * 1024
5. If no matching line found, return None.
```

---

## 7. Files to Create / Modify

### New Files

| File | Purpose |
|------|---------|
| `src/disk.rs` | `detect_available_space`, `parse_df_available`, `format_bytes`, `parse_size_value`, `parse_apt_size`, `parse_dnf_install_size`, `parse_dnf_size_line`, `parse_zypper_size`, `parse_flatpak_download_sizes`, `parse_fwupd_size`, unit tests |

### Modified Files

| File | Changes |
|------|---------|
| `src/main.rs` | Add `mod disk;` |
| `src/backends/mod.rs` | Add `estimate_size()` to `Backend` trait (default `None`) |
| `src/backends/os_package_manager.rs` | Implement `estimate_size()` for `AptBackend`, `DnfBackend`, `ZypperBackend` (Pacman default `None`) |
| `src/backends/flatpak.rs` | Implement `estimate_size()` for `FlatpakBackend` |
| `src/backends/fwupd.rs` | Implement `estimate_size()` for `FwupdBackend` |
| `src/backends/homebrew.rs` | No change (uses trait default `None`) |
| `src/backends/nix.rs` | No change (uses trait default `None`) |
| `src/ui/update_row.rs` | Add `base_description: String` and `last_size_bytes: Rc<Cell<Option<u64>>>` fields; `set_download_size()`; `last_size_bytes()` getter; reset in `set_status_checking()` |
| `src/ui/window.rs` | Extend `run_checks` channel type and payload; add `available_space_bytes`, `bypass_disk_space` state; add disk-space warning dialog in click handler |

---

## 8. Implementation Steps

### Step 1 — Create `src/disk.rs`

Create `src/disk.rs` with:
- `detect_available_space() -> Option<u64>` (async, `pub`)
- `parse_df_available(text: &str) -> Option<u64>` (`pub(crate)`)
- `format_bytes(bytes: u64) -> String` (`pub`)
- `parse_size_value(n: f64, unit: &str) -> u64` (`pub(crate)`)
- `parse_apt_size(output: &str) -> Option<u64>` (`pub(crate)`)
- `parse_dnf_install_size(output: &str) -> Option<u64>` (`pub(crate)`)
- `parse_zypper_size(output: &str) -> Option<u64>` (`pub(crate)`)
- `parse_flatpak_download_sizes(output: &str) -> u64` (`pub(crate)`)
- `parse_fwupd_size(json_text: &str) -> u64` (`pub(crate)`)
- All `#[cfg(test)]` unit tests

### Step 2 — Register module in `src/main.rs`

Add `mod disk;` after `mod config;`.

### Step 3 — Extend `Backend` trait in `src/backends/mod.rs`

Add `estimate_size()` default method returning `Box::pin(async { None })` immediately after `list_available`.

### Step 4 — Implement `estimate_size()` in `src/backends/os_package_manager.rs`

- `AptBackend::estimate_size()` — run `apt-get -s upgrade`, call `crate::disk::parse_apt_size`
- `DnfBackend::estimate_size()` — run `dnf upgrade --assumeno`, call `crate::disk::parse_dnf_install_size`
- `ZypperBackend::estimate_size()` — run `zypper update --dry-run`, call `crate::disk::parse_zypper_size`
- `PacmanBackend` — no override; uses default `None`

### Step 5 — Implement `estimate_size()` in `src/backends/flatpak.rs`

- `FlatpakBackend::estimate_size()` — run `flatpak remote-ls --updates --user --columns=download-size` (with sandbox-aware `build_flatpak_cmd`), call `crate::disk::parse_flatpak_download_sizes`

### Step 6 — Implement `estimate_size()` in `src/backends/fwupd.rs`

- `FwupdBackend::estimate_size()` — re-run `fwupdmgr get-updates --json`, call `crate::disk::parse_fwupd_size`

### Step 7 — Extend `UpdateRow` in `src/ui/update_row.rs`

1. Add `base_description: String` and `last_size_bytes: Rc<Cell<Option<u64>>>` fields.
2. Capture `backend.description().to_string()` in `new()`.
3. Initialise `last_size_bytes: Rc::new(Cell::new(None))`.
4. Include both in `Self { ... }`.
5. Add `set_download_size(size_bytes: Option<u64>)` method.
6. Add `last_size_bytes() -> Option<u64>` getter.
7. In `set_status_checking()`, add: `self.last_size_bytes.set(None); self.row.set_subtitle(&self.base_description);`

### Step 8 — Extend `run_checks` in `src/ui/window.rs`

1. Update `CheckPayload` type alias to include `Option<u64>`.
2. In the background task: add `let size = backend_clone.estimate_size().await;` after `list`.
3. In `tx.send(...)`: include `size`.
4. In the receive handler: destructure `(count_result, list_result, size_result)` and call `row.set_download_size(size_result)`.
5. Add `available_space_bytes: Rc<Cell<Option<u64>>>` state (declare alongside `pending_checks`).
6. When `remaining == 0`: spawn background task to detect available space and store in `available_space_bytes`.

### Step 9 — Add warning dialog in `src/ui/window.rs`

1. Add `bypass_disk_space: Rc<Cell<bool>>` alongside existing bypass flags.
2. Capture `available_space_bytes` and `bypass_disk_space` in `update_button.connect_clicked` via `#[strong]`.
3. Insert the disk-space check block as documented in Section 4.5.3, between battery and snapshot checks.

### Step 10 — Run preflight

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo build
cargo test
```

Fix any issues before considering the feature complete.

---

## 9. Dependencies

**No new Cargo dependencies are introduced.**

| Capability | How Provided |
|-----------|-------------|
| Available space detection | `df -k /` (standard Linux utility) via `tokio::process::Command` |
| APT size estimation | `apt-get -s upgrade` (already used for APT operations) |
| DNF size estimation | `dnf upgrade --assumeno` (standard DNF flag) |
| Zypper size estimation | `zypper update --dry-run` (standard Zypper flag) |
| Flatpak size estimation | `flatpak remote-ls --columns=download-size` (already uses Flatpak) |
| fwupd size estimation | `fwupdmgr get-updates --json` (already used in `list_available`) |
| JSON parsing | `serde_json` (already in `Cargo.toml`) |
| Human-readable formatting | Pure Rust arithmetic (no crate needed) |
| GTK dialog | `adw::AlertDialog` (already used in window.rs) |

---

## 10. Risks & Mitigations

### R1 — DNF `--assumeno` exits non-zero

**Risk**: `dnf upgrade --assumeno` exits with code 1 when upgrades are available (it aborts the transaction). Checking `out.status.success()` would cause the estimate to return `None` even when there are updates.

**Mitigation**: Do not check exit status for DNF's `estimate_size()`. Combine stdout + stderr and parse unconditionally. An empty combined output (no packages) naturally produces `None`. If DNF is not installed or the command cannot be spawned, `.ok()?` handles that case.

### R2 — APT `apt-get -s` locale fallback

**Risk**: Despite `LANG=C`, some APT builds or locales may emit size lines in a different format or with non-breaking spaces.

**Mitigation**: Parse using `split_whitespace()` (which handles all Unicode whitespace), not fixed column positions. The regex is anchored to `"After this operation,"` + `"disk space"` so locale-specific text in the middle is tolerated as long as the prefix and suffix match.

### R3 — Flatpak `--columns=download-size` format changes

**Risk**: The human-readable format of Flatpak size columns may change between versions (e.g., "12.3 MB" vs "12.3MB").

**Mitigation**: Parsing uses `split_whitespace()` (handles no-space), and the numeric parse (`parts[0].parse::<f64>()`) gracefully fails and skips lines it cannot parse. Non-zero sizes are reported; total of 0 is treated as `None`.

### R4 — fwupd JSON schema changes

**Risk**: A future fwupd release changes the JSON schema (e.g., removes or renames `Size`).

**Mitigation**: All JSON field accesses use `.get(…).and_then(…)` → silent `None`. `parse_fwupd_size` returns 0 if the field is absent, and 0 is treated as `None` in `estimate_size()`.

### R5 — `df -k /` line-wrapping on long device names

**Risk**: When the filesystem path exceeds ~20 characters, `df` wraps the data row to a second line. A naive line-split would produce an empty Available field.

**Mitigation**: The `parse_df_available` implementation reconstructs wrapped lines by detecting continuation lines (lines starting with whitespace) and joining them to the previous line before field extraction.

### R6 — False-positive disk warnings (e.g., size overestimation)

**Risk**: APT's "additional disk space" figure includes temporary package cache. If the user has a separate `/var` partition, the reported requirement may be an overestimate for the root partition.

**Mitigation**: The 10% headroom threshold (`× 1.1`) provides a safety buffer. The dialog allows "Update Anyway", so users are not blocked. The warning message explicitly says "may fail" rather than "will fail", setting appropriate expectations.

### R7 — Zypper `--dry-run` modifies zypper lock or cache

**Risk**: `zypper update --dry-run` resolves dependencies and reads repository metadata. On slow systems this could take several seconds.

**Mitigation**: `estimate_size()` runs concurrently with `count_available()` and `list_available()` in the background task. Since `list_available()` already calls `zypper list-updates`, the metadata cache should be warm. The background nature means the UI is non-blocking.

### R8 — Race: available space changes between check and update

**Risk**: Available space is detected during `run_checks` but could decrease between check and clicking "Update All" (e.g., other downloads filling disk).

**Mitigation**: This is a best-effort pre-check. The warning threshold includes 10% headroom to absorb moderate changes. The OS package manager will fail gracefully if space truly runs out during installation.

### R9 — `estimate_size()` called on NixOS or Homebrew systems

**Risk**: These backends return `None` from `estimate_size()`, and their rows will not have size data. If they are the only backends, the `total_required_bytes` will be 0 and no warning will fire.

**Mitigation**: This is by design. The spec explicitly states graceful degradation: backends that cannot estimate size simply omit it from the UI. The "Update Anyway" flow is always available. A future enhancement could add Nix support via `nix build --dry-run` when that stabilises.

### R10 — Warning dialog appears incorrectly when "freed" space is reported

**Risk**: APT and Zypper can report that the operation will *free* disk space (e.g., when removing packages). In this case `required_bytes` would be 0 and no warning is shown.

**Mitigation**: Both `parse_apt_size` and `parse_zypper_size` explicitly return `Some(0)` when the output contains "freed". In `estimate_size()`, a return of `Some(0)` is passed to `set_download_size(Some(0))`, which falls into the `_ => self.base_description.clone()` branch (no size shown in subtitle). The accumulator adds 0, so no warning fires. This is correct behaviour.

---

## Summary

This specification introduces a complete, non-breaking disk-space estimation layer to Up:

1. **`src/disk.rs`** (new) — all parsing utilities, `detect_available_space`, `format_bytes`
2. **`src/backends/mod.rs`** — one new optional trait method `estimate_size()`
3. **Five backend implementations** — APT, DNF, Zypper, Flatpak, fwupd
4. **`src/ui/update_row.rs`** — subtitle updated with estimated size; new getter
5. **`src/ui/window.rs`** — check cycle extended; warning dialog added

The design follows all existing conventions: `Rc<Cell<_>>` for GTK-thread state, `async_channel` for background→GTK communication, `adw::AlertDialog` with bypass flags for pre-flight warnings, `tokio::process::Command` for background subprocesses, and `LANG=C` for locale-stable output parsing. No new Cargo dependencies are required.
