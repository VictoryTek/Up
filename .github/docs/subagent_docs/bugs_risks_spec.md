# Bugs & Risks Fix Specification
**Project:** Up — GTK4/libadwaita Linux desktop application  
**Scope:** Section 2 bugs and risks from CODEBASE_ANALYSIS.md  
**Priority order:** HIGH → MEDIUM → LOW

---

## Summary of Files to Modify

| File | Issues |
|------|--------|
| `src/ui/upgrade_page.rs` | 3.4 |
| `src/ui/window.rs` | 3.5, 3.18 |
| `src/backends/nix.rs` | 3.3 |
| `src/upgrade.rs` | 3.6, 3.12, 3.15, 3.19 |
| `src/backends/os_package_manager.rs` | 3.14 |
| `src/reboot.rs` | 3.10 |
| `src/ui/reboot_dialog.rs` | 3.10 |
| `src/backends/flatpak.rs` | 3.13, 3.20 |

**New crate dependencies:** None required (mktemp shell approach for 3.20 avoids `tempfile` crate).

---

## HIGH Severity

---

### Issue 3.4 — `src/ui/upgrade_page.rs`: `.expect()` panics GTK main loop

#### Current State

Two `.expect()` calls inside GTK signal handlers in `upgrade_page.rs`. If the `Option` is `None` (which can happen due to race conditions or UI state bugs), the process panics, crashing the GTK main loop.

**Location 1** — `check_button.connect_clicked` handler (approximately line 160):
```rust
check_button.connect_clicked(move |button| {
    let distro = distro_state_for_check
        .borrow()
        .clone()
        .expect("distro info must be available before check button is sensitive");
    button.set_sensitive(false);
```

**Location 2** — `upgrade_button.connect_clicked` handler (approximately line 241):
```rust
upgrade_button.connect_clicked(move |button| {
    let distro = distro_state_for_upgrade
        .borrow()
        .clone()
        .expect("distro info must be available before upgrade button is active");
```

#### Proposed Fix

Replace both `.expect()` calls with `if let Some(...) = ... else { return; }` guards. This matches the established GTK signal-handler pattern in the codebase (e.g., `window.rs` `about_action` handler).

**Fix for Location 1:**
```rust
check_button.connect_clicked(move |button| {
    let Some(distro) = distro_state_for_check.borrow().clone() else {
        return;
    };
    button.set_sensitive(false);
```

**Fix for Location 2:**
```rust
upgrade_button.connect_clicked(move |button| {
    let Some(distro) = distro_state_for_upgrade.borrow().clone() else {
        return;
    };
```

#### Affected Files
- `src/ui/upgrade_page.rs`

#### Risks and Edge Cases
- **Risk:** Both buttons are already gated (set `sensitive(false)` until distro info arrives), so the `None` case is unlikely in normal operation. However, a future refactor could break this invariant. The guard makes the contract explicit and safe.
- **Edge case:** If button is clicked and `distro` is `None`, the UI stays responsive (button remains sensitive) — this is better than a crash. If desired, the button could be explicitly set to `sensitive(false)` before returning, but the current gate mechanism already handles this.

---

### Issue 3.5 — `src/ui/window.rs`: Index-captured row access panics if backend list mutates

#### Current State

Inside the `run_checks` closure in `build_update_page()`, the outer loop uses `enumerate()` to capture `idx`, and then the spawned async future uses `rows_ref.borrow()[idx]` to look up the row:

```rust
for (idx, backend) in detected.borrow().iter().enumerate() {
    {
        let borrowed = rows.borrow();
        borrowed[idx].1.set_status_checking();  // index-based access
    }
    // ...
    let rows_ref = rows.clone();
    // idx is captured by move into the async block
    glib::spawn_future_local(async move {
        // ...
        if let Ok((count_result, list_result)) = rx.recv().await {
            if epoch_ref.get() != my_epoch { return; }
            let row = rows_ref.borrow()[idx].1.clone();  // panics if rows shrinks
```

If the `rows` Vec were to shrink (e.g., due to a concurrent re-detection cycle), `rows_ref.borrow()[idx]` will panic with an index-out-of-bounds. In addition, the loop also sets `borrowed[idx].1.set_status_checking()` with the same risk.

Note: the `update_button.connect_clicked` handler already uses the safe pattern — `rows_borrowed.iter().find(|(k, _)| *k == kind)` — introduced in a previous fix. The `run_checks` closure was not updated to match.

#### Proposed Fix

Capture `BackendKind` instead of `idx` in the loop body. Look up rows by kind instead of by index, using the same `.iter().find()` idiom already used in the update button handler.

**Replace the `run_checks` for-loop from:**
```rust
for (idx, backend) in detected.borrow().iter().enumerate() {
    {
        let borrowed = rows.borrow();
        borrowed[idx].1.set_status_checking();
    }
    let backend_clone = backend.clone();
    let rows_ref = rows.clone();
    let pending_ref = pending_checks.clone();
    let total_ref = total_available.clone();
    let btn_ref = update_button_checks.clone();
    let status_ref = status_label_checks.clone();
    let epoch_ref = check_epoch.clone();
    glib::spawn_future_local(async move {
        type CheckPayload = (Result<usize, String>, Result<Vec<String>, String>);
        let (tx, rx) = async_channel::bounded::<CheckPayload>(1);
        super::spawn_background_async(move || async move {
            let count = backend_clone.count_available().await;
            let list = backend_clone.list_available().await;
            let _ = tx.send((count, list)).await;
        });
        if let Ok((count_result, list_result)) = rx.recv().await {
            // Discard results from a superseded check cycle.
            if epoch_ref.get() != my_epoch {
                return;
            }
            let row = rows_ref.borrow()[idx].1.clone();
            match count_result {
```

**To (full replacement of the loop body):**
```rust
for backend in detected.borrow().iter() {
    let kind = backend.kind();
    {
        let borrowed = rows.borrow();
        if let Some((_, row)) = borrowed.iter().find(|(k, _)| *k == kind) {
            row.set_status_checking();
        }
    }
    let backend_clone = backend.clone();
    let rows_ref = rows.clone();
    let pending_ref = pending_checks.clone();
    let total_ref = total_available.clone();
    let btn_ref = update_button_checks.clone();
    let status_ref = status_label_checks.clone();
    let epoch_ref = check_epoch.clone();
    glib::spawn_future_local(async move {
        type CheckPayload = (Result<usize, String>, Result<Vec<String>, String>);
        let (tx, rx) = async_channel::bounded::<CheckPayload>(1);
        super::spawn_background_async(move || async move {
            let count = backend_clone.count_available().await;
            let list = backend_clone.list_available().await;
            let _ = tx.send((count, list)).await;
        });
        if let Ok((count_result, list_result)) = rx.recv().await {
            // Discard results from a superseded check cycle.
            if epoch_ref.get() != my_epoch {
                return;
            }
            let row = {
                let borrowed = rows_ref.borrow();
                borrowed.iter().find(|(k, _)| *k == kind).map(|(_, r)| r.clone())
            };
            let Some(row) = row else { return; };
            match count_result {
```

The remainder of the loop body (where `row.set_status_available(count)`, etc. are called) is unchanged; only the `let row = ...` assignment changes from index-based to kind-based lookup.

#### Affected Files
- `src/ui/window.rs`

#### Risks and Edge Cases
- **Risk:** Rows are populated once during detection (in `glib::spawn_future_local` that processes the detection result) and never mutated afterwards in the current architecture. So the panic is unlikely in practice, but the code has no invariant enforcing this. The kind-based lookup is strictly safer.
- **Edge case:** If a backend has no corresponding row (which should never happen in normal flow), the future now silently returns instead of panicking — better failure mode.

---

### Issue 3.3 — `src/backends/nix.rs`: Flatpak sandbox paths break NixOS/Determinate Nix detection

#### Current State

Three probe functions check host-only filesystem paths but do so from within the Flatpak sandbox, where those paths refer to sandbox content, not the host:

- `is_nixos()` — checks `/run/current-system`, `/etc/os-release`, `/etc/nixos`
- `is_nixos_flake()` — checks `/etc/nixos/flake.nix`
- `is_determinate_nix()` — checks `/nix/receipt.json`, `which::which("determinate-nixd")`

When Up runs as a Flatpak, `/run/current-system` does not exist inside the sandbox even on a NixOS host. The Nix backend silently shows as unavailable.

`is_running_in_flatpak()` already exists as a public function in `src/backends/flatpak.rs` and checks `/.flatpak-info`. It can be referenced from `nix.rs` as `crate::backends::flatpak::is_running_in_flatpak()` (same crate, no circular dependency).

`flatpak-spawn --host` already used elsewhere in the codebase for host commands. For detection, `flatpak-spawn --host test -e <path>` exits 0 if the path exists on the host.

#### Proposed Fix

Wrap all three probe functions: when inside Flatpak, delegate to `flatpak-spawn --host test -e <path>` for each path check, and to `flatpak-spawn --host which <binary>` for binary checks.

**`is_nixos()` — full replacement:**
```rust
fn is_nixos() -> bool {
    if crate::backends::flatpak::is_running_in_flatpak() {
        // Inside the Flatpak sandbox, probe the host filesystem via flatpak-spawn.
        // /run/current-system is the most reliable NixOS-specific indicator.
        return std::process::Command::new("flatpak-spawn")
            .args(["--host", "test", "-e", "/run/current-system"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    }
    if std::path::Path::new("/run/current-system").exists() {
        return true;
    }
    if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
        if content.lines().any(|l| l.trim() == "ID=nixos") {
            return true;
        }
    }
    std::path::Path::new("/etc/nixos").exists()
}
```

**`is_nixos_flake()` — full replacement:**
```rust
fn is_nixos_flake() -> bool {
    if crate::backends::flatpak::is_running_in_flatpak() {
        return std::process::Command::new("flatpak-spawn")
            .args(["--host", "test", "-e", "/etc/nixos/flake.nix"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    }
    std::path::Path::new("/etc/nixos/flake.nix").exists()
}
```

**`is_determinate_nix()` — full replacement:**
```rust
fn is_determinate_nix() -> bool {
    if crate::backends::flatpak::is_running_in_flatpak() {
        let receipt_ok = std::process::Command::new("flatpak-spawn")
            .args(["--host", "test", "-e", "/nix/receipt.json"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let daemon_ok = std::process::Command::new("flatpak-spawn")
            .args(["--host", "which", "determinate-nixd"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        return receipt_ok && daemon_ok;
    }
    std::path::Path::new("/nix/receipt.json").exists() && which::which("determinate-nixd").is_ok()
}
```

#### Affected Files
- `src/backends/nix.rs`

#### Risks and Edge Cases
- **Risk:** `flatpak-spawn --host test -e <path>` requires the `--talk-name=org.freedesktop.Flatpak` D-Bus permission (or `--host` access). This is already a required permission for the existing `flatpak-spawn --host` calls in the codebase.
- **Risk:** Each probe spawns an external process, adding latency. `is_nixos()` is called from `description()`, `needs_root()`, and `run_update()`. These are all called at startup, not in tight loops, so the overhead is acceptable.
- **Edge case:** If `flatpak-spawn` is not available inside the sandbox (unusual), all probes return `false`. This is the same safe default as the non-Flatpak path on a non-NixOS system.

---

## MEDIUM Severity

---

### Issue 3.6 — `src/upgrade.rs`: Ubuntu tail thread leaks; `drop(tail_handle)` does not terminate threads

#### Current State

`upgrade_ubuntu()` spawns a `tail_handle` thread that tails `/var/log/dist-upgrade/main.log` indefinitely via a `loop { ... }` with `sleep(500ms)`. After the upgrade command returns, the code does:

```rust
drop(tail_handle);
result
```

`drop` on a `JoinHandle` abandons the thread — it does NOT send a signal or interrupt the thread. The tail thread keeps running until the process exits. If the Ubuntu upgrade function is called multiple times (even theoretically), threads accumulate.

#### Proposed Fix

Introduce an `Arc<AtomicBool>` cancellation flag. The tail thread checks it after each sleep; when set, the thread exits. The main function sets it to `true` after the upgrade command completes.

**Required import at top of `upgrade.rs`** (already has `std::sync::Arc` via `use std::sync::Arc` in other files, but add atomics):
```rust
use std::sync::atomic::{AtomicBool, Ordering};
```

**Replace the tail_handle section in `upgrade_ubuntu()`:**

Current:
```rust
let log_path = "/var/log/dist-upgrade/main.log";
let tx_tail = tx.clone();
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

let result = if !crate::runner::run_command_sync( ...

drop(tail_handle);
result
```

Replacement:
```rust
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

let result = if !crate::runner::run_command_sync( ...
// Set cancellation flag so the tail thread exits its loop.
cancel_flag.store(true, Ordering::Relaxed);
// Wait for the tail thread to finish draining any remaining lines.
let _ = tail_handle.join();
result
```

Note: `std::sync::atomic` is part of std and requires no new imports beyond adding the use statement. `Arc` is already imported via `std::sync::Arc`.

#### Affected Files
- `src/upgrade.rs`

#### Risks and Edge Cases
- **Risk:** The tail thread checks the flag after sleeping 500ms, so there is at most a ~500ms delay between setting the flag and the thread exiting. This is acceptable.
- **Risk:** `tail_handle.join()` will block until the thread exits. If the thread is stuck on `read_line` when an I/O error occurs, the `Err(_) => break` clause handles it. If `read_line` blocks indefinitely (unusual for a regular file), this could stall. In practice `read_line` on a file returns `Ok(0)` when at EOF, not blocks.

---

### Issue 3.14 — `src/backends/os_package_manager.rs`: DNF `count_available` misinterprets exit codes

#### Current State

`DnfBackend::count_available()` handles exit codes:
- Code 0 → `Ok(0)` ✓
- Any other code → parses output and counts lines

But DNF's documented exit codes are:
- 0 → up to date (no updates)
- 1 → error/failure
- 100 → updates are available

The current code treats exit code 1 (a real DNF error) the same as exit code 100 (updates available), causing the Fedora row to show a spurious update count on DNF errors.

Current code:
```rust
fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
    Box::pin(async move {
        let out = tokio::process::Command::new("dnf")
            .args(["check-update"])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        if out.status.code() == Some(0) {
            return Ok(0);
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let count = text
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with("Last") && !l.starts_with("Obsoleting"))
            .count();
        Ok(count)
    })
}
```

Note: `list_available()` already correctly handles exit code 1 by returning `Err`.

#### Proposed Fix

Match specifically on exit code 100 for "updates available", treat code 1 as an error, and treat anything else as no updates (safe default):

```rust
fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
    Box::pin(async move {
        let out = tokio::process::Command::new("dnf")
            .args(["check-update"])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        match out.status.code() {
            Some(0) => return Ok(0),   // No updates available
            Some(1) => return Err("dnf check-update failed".to_string()), // DNF error
            Some(100) => {}            // Updates available — continue to count
            _ => return Ok(0),         // Unknown exit code, safe default
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let count = text
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with("Last") && !l.starts_with("Obsoleting"))
            .count();
        Ok(count)
    })
}
```

#### Affected Files
- `src/backends/os_package_manager.rs`

#### Risks and Edge Cases
- **Risk:** DNF5 (Fedora 41+) uses the same exit codes as DNF4 (0, 1, 100). The fix is backwards compatible.
- **Edge case:** Some older Fedora/RHEL versions may differ, but exit code 100 for "updates available" has been stable since DNF3.

---

### Issue 3.10 — `src/reboot.rs`: `systemctl reboot` failure not surfaced to user

#### Current State

`reboot()` uses `Command::spawn()` (fire-and-forget). Errors from `spawn()` (process could not be started) are only logged to stderr — not shown to the user:

```rust
pub fn reboot() {
    info!("Reboot requested");
    if Path::new("/.flatpak-info").exists() {
        if let Err(e) = Command::new("flatpak-spawn")
            .args(["--host", "systemctl", "reboot"])
            .spawn()
        {
            error!("Failed to spawn reboot command: {e}");
        }
    } else if let Err(e) = Command::new("systemctl").arg("reboot").spawn() {
        error!("Failed to spawn reboot command: {e}");
    }
}
```

If systemctl returns a non-zero exit status (e.g., under Flatpak with insufficient D-Bus permissions), this is also invisible to the user.

#### Proposed Fix

**Step 1:** Change `reboot()` to return `Result<(), String>` and use `.status()` (blocking) instead of `.spawn()`. Because a successful reboot kills the process before `.status()` returns, the blocking call only "completes" in the failure case:

```rust
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
```

**Step 2:** Update `reboot_dialog.rs` to call `reboot()` from a background thread (to avoid blocking the GTK main loop) and show an `adw::AlertDialog` on failure:

```rust
dialog.connect_response(None, move |_dialog, response| {
    if response == "reboot" {
        let (err_tx, err_rx) = async_channel::bounded::<String>(1);

        // Run in a background thread because `reboot()` blocks on .status().
        // On successful reboot systemd kills the process before it can return.
        // On failure, the error is sent back to the GTK main loop via the channel.
        std::thread::spawn(move || {
            if let Err(e) = crate::reboot::reboot() {
                let _ = err_tx.send_blocking(e);
            }
        });

        glib::spawn_future_local(async move {
            if let Ok(err_msg) = err_rx.recv().await {
                let error_dialog = adw::AlertDialog::builder()
                    .heading("Reboot Failed")
                    .body(format!(
                        "The system could not be rebooted.\n\n{err_msg}\n\n\
                         Please reboot manually using your system settings or terminal."
                    ))
                    .build();
                error_dialog.add_response("close", "Close");
                error_dialog.set_default_response(Some("close"));
                error_dialog.set_close_response("close");
                // Note: no parent widget available in this closure; present without parent.
                error_dialog.present(None::<&gtk::Widget>);
            }
        });
    }
});
```

The `use gtk::glib;` import is already present at the top of `reboot_dialog.rs` (inherits from `adw::prelude::*`). Add `use gtk::glib;` explicitly if needed.

#### Affected Files
- `src/reboot.rs`
- `src/ui/reboot_dialog.rs`

#### Risks and Edge Cases
- **Risk:** `.status()` blocks the spawned thread until the reboot command exits. On successful reboot, the process is killed by systemd. The background thread is killed along with the process, so there's no resource leak.
- **Risk:** Showing `error_dialog.present(None::<&gtk::Widget>)` means the dialog has no parent window. This is acceptable for an error dialog; GTK will still display it. If a parent reference is available in the closure, pass it.
- **Edge case:** Under Flatpak, `flatpak-spawn --host systemctl reboot` may fail if the Flatpak manifest does not include `--talk-name=org.freedesktop.systemd1`. This is the failure case this fix is specifically designed to surface.

---

### Issue 3.12 — `src/upgrade.rs`: `check_packages_up_to_date` does not force `LANG=C`

#### Current State

`check_packages_up_to_date()` runs package manager commands and parses their output without setting `LANG=C`:

```rust
match Command::new(cmd).args(args).output() {
    Ok(output) => {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let upgradable = stdout
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with("Listing"))
            .count();
```

On non-English locales, `apt list --upgradable` emits a different header (e.g., German: "Auflistung..."), causing the `!l.starts_with("Listing")` filter to fail and all lines (including the header) to be counted as upgradable packages — falsely reporting the system as out of date.

Similarly, `dnf check-update` and `zypper list-updates` emit locale-dependent headers and section labels.

#### Proposed Fix

Add `.env("LANG", "C").env("LC_ALL", "C")` to the `Command` builder. Both `LANG` and `LC_ALL` must be set because `LC_ALL` overrides `LANG` and must also be reset for full effect:

```rust
match Command::new(cmd).args(args)
    .env("LANG", "C")
    .env("LC_ALL", "C")
    .output()
{
```

This forces all parsed subprocess output to English regardless of the user's locale. The fix is a single-line addition to the `Command` builder chain.

Additionally, to be thorough: the `fetch_ubuntu_meta_release()` function uses `curl` which is not locale-sensitive (HTTP response content is fixed), so it does not need this fix. The `df --output=avail -B1 /` in `check_disk_space()` uses `--output=avail` which is a column selector, not locale-sensitive text — no change needed there.

#### Affected Files
- `src/upgrade.rs`

#### Risks and Edge Cases
- **Risk:** Setting `LANG=C` and `LC_ALL=C` affects all output including error messages. Error messages will be in English, which is acceptable for log output.
- **Edge case:** Package names containing UTF-8 are not affected by `LANG=C`; only textual messages and headers change.

---

### Issue 3.19 — `src/upgrade.rs`: `upgrade_nixos` uses hostname instead of `resolve_nixos_flake_attr()`

#### Current State

`upgrade_nixos()` (in `upgrade.rs`) handles the flake branch by detecting the hostname and building a flake reference `"/etc/nixos#<hostname>"`:

```rust
NixOsConfigType::Flake => {
    // ...
    let raw_hostname = detect_hostname();
    let hostname = match validate_hostname(&raw_hostname) {
        Ok(h) => h,
        Err(e) => {
            let msg = format!("Upgrade aborted: {e}");
            let _ = tx.send_blocking(msg.clone());
            return Err(msg);
        }
    };
    let flake_target = format!("/etc/nixos#{}", hostname);
    let _ = tx.send_blocking(format!("Rebuilding NixOS configuration: {flake_target}"));
    if !crate::runner::run_command_sync(
        "pkexec",
        &["nixos-rebuild", "switch", "--flake", &flake_target],
        tx,
    ) {
```

Meanwhile, `NixBackend::run_update()` in `src/backends/nix.rs` correctly uses `resolve_nixos_flake_attr()` (which reads `/etc/nixos/vexos-variant`) to get the flake attribute name. This is more reliable because hostnames may not match NixOS configuration attribute names (e.g., hostname is "vexos" but config is "vexos-nvidia").

`resolve_nixos_flake_attr()` is currently a private `fn` in `nix.rs`.

#### Proposed Fix

**Step 1:** Make `resolve_nixos_flake_attr()` in `src/backends/nix.rs` visible to the crate by changing the function signature from `fn` to `pub(crate) fn`:

```rust
// Before:
fn resolve_nixos_flake_attr() -> Result<String, String> {

// After:
pub(crate) fn resolve_nixos_flake_attr() -> Result<String, String> {
```

**Step 2:** In `upgrade_nixos()` in `src/upgrade.rs`, replace the hostname-based flake attribute resolution with `crate::backends::nix::resolve_nixos_flake_attr()`:

Replace:
```rust
let raw_hostname = detect_hostname();
let hostname = match validate_hostname(&raw_hostname) {
    Ok(h) => h,
    Err(e) => {
        let msg = format!("Upgrade aborted: {e}");
        let _ = tx.send_blocking(msg.clone());
        return Err(msg);
    }
};
let flake_target = format!("/etc/nixos#{}", hostname);
let _ = tx.send_blocking(format!("Rebuilding NixOS configuration: {flake_target}"));
```

With:
```rust
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
```

The `config_attr` is already validated by `resolve_nixos_flake_attr()` via `validate_flake_attr()` (ASCII alphanumeric / hyphen / underscore / dot only), so it is safe to interpolate into the flake reference string.

#### Affected Files
- `src/backends/nix.rs` (visibility change only)
- `src/upgrade.rs`

#### Risks and Edge Cases
- **Risk:** `resolve_nixos_flake_attr()` reads `/etc/nixos/vexos-variant` — this is a VexOS-specific convention. On other NixOS systems where this file doesn't exist, `resolve_nixos_flake_attr()` returns an `Err` with instructions for creating the file. The error will be shown in the upgrade log UI, which is acceptable.
- **Edge case:** The function was already used in `NixBackend::run_update()` for the same purpose, so the consistency is important — both update paths should use the same resolution mechanism.
- **Note:** The `detect_hostname()` and `validate_hostname()` functions in `upgrade.rs` are still used elsewhere (e.g., in `window.rs` for the nixos_extra detection), so they should NOT be removed.

---

### Issue 3.18 — `src/ui/window.rs`: Refresh button not disabled during updates

#### Current State

The refresh button is wired directly to `run_checks()` with no guard:

```rust
let run_checks_btn = run_checks.clone();
refresh_button.connect_clicked(move |_| (*run_checks_btn)());
```

If the user clicks "Update All" and then clicks the refresh button while the update is running, the check cycle starts in parallel with the ongoing update. Both will concurrently access `rows` (the `Rc<RefCell<Vec<(BackendKind, UpdateRow)>>>`) and call widget methods, which while technically safe (single-threaded GTK), produces confusing UI state.

#### Proposed Fix

Return an `Rc<Cell<bool>>` update-in-progress flag from `build_update_page()`, and use it in `UpWindow::build()` to guard the refresh button and update button.

**Step 1:** Add `updating_flag: Rc<Cell<bool>>` to the return type of `build_update_page()`:

Change signature from:
```rust
fn build_update_page() -> (gtk::Box, Rc<dyn Fn()>, adw::ActionRow, adw::ActionRow)
```
To:
```rust
fn build_update_page() -> (gtk::Box, Rc<dyn Fn()>, adw::ActionRow, adw::ActionRow, Rc<Cell<bool>>)
```

**Step 2:** Inside `build_update_page()`, create the flag:
```rust
let updating: Rc<Cell<bool>> = Rc::new(Cell::new(false));
```

**Step 3:** At the start of `update_button.connect_clicked`, set the flag and disable the button (the button is already disabled via `button.set_sensitive(false)`):
```rust
update_button.connect_clicked(move |button| {
    button.set_sensitive(false);
    updating_for_btn.set(true);   // <-- add this
    log_clone.clear();
    // ...
```

**Step 4:** At the end of the update event loop in the `glib::spawn_future_local` block (just before `button_ref.set_sensitive(true)`), clear the flag:
```rust
    // ... end of while let Ok(event) loop ...
    updating_for_btn_ref.set(false);  // <-- add this
    button_ref.set_sensitive(true);
    // ...
```

**Step 5:** Return the flag from `build_update_page()`:
```rust
(page_box, run_checks, distro_row, version_row, updating)
```

**Step 6:** In `UpWindow::build()`, destructure the new return value and wire the refresh button with a guard:
```rust
let (update_page, run_checks, sysinfo_distro_row, sysinfo_version_row, update_in_progress) =
    Self::build_update_page();
```

Then wrap the refresh button handler:
```rust
let run_checks_btn = run_checks.clone();
let update_in_progress_ref = update_in_progress.clone();
refresh_button.connect_clicked(move |_| {
    if update_in_progress_ref.get() {
        return; // silently ignore clicks during active update
    }
    (*run_checks_btn)()
});
```

For visual feedback, optionally disable the refresh button when update starts and re-enable when done. This requires cloning the refresh button into `build_update_page()` or returning it. The simpler option (just ignoring clicks with no visual change) avoids changing more function signatures and is the minimal safe fix.

#### Affected Files
- `src/ui/window.rs`

#### Risks and Edge Cases
- **Risk:** `Rc<Cell<bool>>` is single-threaded, which is correct here — all GTK operations happen on the main thread.
- **Edge case:** The `run_checks` closure itself is not guarded, so the initial automatic check after detection still runs. This is correct behaviour — that check runs before any update could be initiated.

---

### Issue 3.20 — `src/backends/flatpak.rs`: Predictable `/tmp` path for self-update bundle

#### Current State

```rust
const SELF_UPDATE_TMP_PATH: &str = "/tmp/up-self-update.flatpak";
```

Used in `download_and_install_bundle()`:
```rust
let script = format!(
    "curl -fsSL --connect-timeout 10 --max-time 300 -o '{tmp}' '{url}' && \
     flatpak install --bundle --reinstall --user -y '{tmp}'; \
     rm -f '{tmp}'",
    tmp = SELF_UPDATE_TMP_PATH,
    url = url,
);
```

`/tmp/up-self-update.flatpak` is a predictable fixed path in world-writable `/tmp`. A local attacker could pre-create this path as a symlink to an arbitrary location and cause `curl` to overwrite it, or replace the file between download and install — a TOCTOU vulnerability.

`tempfile` crate is **not** in `Cargo.toml` and adding it is unnecessary because the download is already done inside a bash script run via `flatpak-spawn --host`. Shell's `mktemp` command creates files securely with O_EXCL semantics in the target directory.

#### Proposed Fix

Remove the `SELF_UPDATE_TMP_PATH` constant and use `mktemp` inside the bash script. Use `$XDG_RUNTIME_DIR` (user-private `/run/user/<uid>`) with `/tmp` as fallback:

**Remove:**
```rust
const SELF_UPDATE_TMP_PATH: &str = "/tmp/up-self-update.flatpak";
```

**Replace `download_and_install_bundle()` script string:**

Current:
```rust
let script = format!(
    "curl -fsSL --connect-timeout 10 --max-time 300 -o '{tmp}' '{url}' && \
     flatpak install --bundle --reinstall --user -y '{tmp}'; \
     rm -f '{tmp}'",
    tmp = SELF_UPDATE_TMP_PATH,
    url = url,
);
```

Replacement:
```rust
let script = format!(
    "tmp=$(mktemp \"${{XDG_RUNTIME_DIR:-/tmp}}/up-self-update-XXXXXX.flatpak\") \
     && curl -fsSL --connect-timeout 10 --max-time 300 -o \"$tmp\" '{url}' \
     && flatpak install --bundle --reinstall --user -y \"$tmp\"; \
     rm -f \"$tmp\"",
    url = url,
);
```

Key security properties:
- `mktemp` uses O_EXCL — creates the file atomically, fails if path already exists.
- `$XDG_RUNTIME_DIR` is a user-private directory (mode 0700) owned by the session user; it is not world-writable.
- `/tmp` fallback only applies when `$XDG_RUNTIME_DIR` is unset (unusual on systemd systems).
- The `url` variable is already validated by `starts_with(GITHUB_RELEASE_DOWNLOAD_PREFIX)` and is rejected if it contains `'`, so single-quoting in the bash script is safe.
- The temp file path is stored in `$tmp` within the shell script — it is never exposed outside the script and cannot be predicted by other processes.

#### Affected Files
- `src/backends/flatpak.rs`

#### Risks and Edge Cases
- **Risk:** `$XDG_RUNTIME_DIR` may be unavailable in some Flatpak environments. The `:-/tmp` fallback handles this.
- **Edge case:** The `mktemp` command is part of GNU coreutils and is present on all mainstream Linux distros.
- **Note:** No new crate dependency required.

---

## LOW Severity

---

### Issue 3.15 — `src/upgrade.rs`: Fedora `dnf system-upgrade reboot` output discarded

#### Current State

In `upgrade_fedora()`, the `dnf system-upgrade reboot` command is spawned with all I/O redirected to null:

```rust
let _ = tx.send_blocking("Download complete. Scheduling upgrade for next reboot...".into());
use std::process::Stdio;
match std::process::Command::new("pkexec")
    .args(["dnf", "system-upgrade", "reboot"])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
```

The comment explains why we spawn instead of waiting: if the reboot succeeds, systemd kills the process before the command exits. However, there is no reason to discard stdout/stderr — if the command fails before rebooting, the error output would be useful in the log.

#### Proposed Fix

Remove the `Stdio::null()` redirections and instead pipe stdout and stderr, forwarding them to `tx` in a background thread. The thread is fire-and-forget (it will be killed with the process on reboot):

```rust
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
        for line in BufReader::new(stdout).lines().flatten() {
            let _ = tx_out.send_blocking(line);
        }
    });
}
if let Some(stderr) = child.stderr.take() {
    let tx_err = tx.clone();
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        for line in BufReader::new(stderr).lines().flatten() {
            let _ = tx_err.send_blocking(format!("[stderr] {line}"));
        }
    });
}

let _ = tx.send_blocking(
    "Upgrade reboot triggered. The system will restart to apply the upgrade.".into(),
);
Ok(())
```

The match arm returning `Ok(child)` replaces the previous `Ok(_child) => { ... Ok(()) }` arm.

#### Affected Files
- `src/upgrade.rs`

#### Risks and Edge Cases
- **Risk:** If the system reboots successfully, the background threads are killed by the OS. This is expected and acceptable.
- **Risk:** The previous code returned `Ok(())` after a successful `spawn()`. This fix also returns `Ok(())` immediately after spawning (without waiting), preserving the same semantics.

---

### Issue 3.13 — `src/backends/flatpak.rs`: `list_available` uses fragile column-position parsing

#### Current State

`FlatpakBackend::list_available()` uses `flatpak update --no-deploy -y --user` and parses the tabular output by positional token:

```rust
let mut tokens = t.split_whitespace();
if let Some(index) = tokens.next() {
    if index.starts_with(|c: char| c.is_ascii_digit()) && index.ends_with('.') {
        if let Some(app_id) = tokens.next() {
            if !apps.contains(&app_id.to_string()) {
                apps.push(app_id.to_string());
            }
        }
    }
}
```

This relies on Flatpak's default table format: column 0 is a numeric index, column 1 is the application ID. If Flatpak adds, removes, or reorders columns in a future release, this silently produces wrong results.

#### Proposed Fix

Add `--columns=application` to the `flatpak update --no-deploy` command. This instructs Flatpak to output only the application ID column, one per line. The parsing becomes a simple line filter:

**Replace the `build_flatpak_cmd` call and parsing in `list_available()`:**

Current:
```rust
let (cmd, args) = build_flatpak_cmd(&["update", "--no-deploy", "-y", "--user"]);
let out = tokio::process::Command::new(&cmd)
    .args(&args)
    .output()
    .await
    .map_err(|e| e.to_string())?;

if !out.status.success() {
    let stderr = String::from_utf8_lossy(&out.stderr);
    return Err(format!("flatpak update --no-deploy failed: {stderr}"));
}

let text = String::from_utf8_lossy(&out.stdout);
let mut apps: Vec<String> = Vec::new();
for line in text.lines() {
    let t = line.trim();
    let mut tokens = t.split_whitespace();
    if let Some(index) = tokens.next() {
        if index.starts_with(|c: char| c.is_ascii_digit()) && index.ends_with('.') {
            if let Some(app_id) = tokens.next() {
                if !apps.contains(&app_id.to_string()) {
                    apps.push(app_id.to_string());
                }
            }
        }
    }
}
Ok(apps)
```

Replacement:
```rust
let (cmd, args) = build_flatpak_cmd(&[
    "update", "--no-deploy", "-y", "--user", "--columns=application",
]);
let out = tokio::process::Command::new(&cmd)
    .args(&args)
    .output()
    .await
    .map_err(|e| e.to_string())?;

if !out.status.success() {
    let stderr = String::from_utf8_lossy(&out.stderr);
    return Err(format!("flatpak update --no-deploy failed: {stderr}"));
}

let text = String::from_utf8_lossy(&out.stdout);
let mut apps: Vec<String> = Vec::new();
for line in text.lines() {
    let t = line.trim();
    // --columns=application produces one app ID per line.
    // Skip the "Application" header and empty lines.
    if t.is_empty() || t.eq_ignore_ascii_case("application") {
        continue;
    }
    // App IDs follow the reverse-DNS convention and contain dots.
    if t.contains('.') && !apps.contains(&t.to_string()) {
        apps.push(t.to_string());
    }
}
Ok(apps)
```

#### Affected Files
- `src/backends/flatpak.rs`

#### Risks and Edge Cases
- **Risk:** `--columns` was introduced in Flatpak 1.2.0 (2019). All supported distros ship a newer version.
- **Risk:** The header text when using `--columns=application` is "Application". The `t.eq_ignore_ascii_case("application")` guard handles it regardless of case.
- **Edge case:** App IDs that do not contain `.` are non-standard but theoretically possible. The `.contains('.')` filter might skip them. However, all legitimate Flatpak app IDs follow reverse-DNS and contain dots; this filter is safe in practice.

---

## Dependency Changes

None. No new crate dependencies are required:
- Issue 3.20 uses shell `mktemp` instead of the `tempfile` crate.
- All other fixes use `std` types (`Arc`, `AtomicBool`, `Cell`) and existing crate imports.

`Cargo.toml` does **not** need to be modified.

---

## Implementation Order

Implement in this order to minimize merge conflicts (files touched multiple times):

1. `src/ui/upgrade_page.rs` — Issue 3.4 (two `.expect()` replacements)
2. `src/ui/window.rs` — Issues 3.5 and 3.18 (both in the same file; do together)
3. `src/backends/nix.rs` — Issues 3.3 and 3.19 (both in the same file; do together)
4. `src/upgrade.rs` — Issues 3.6, 3.12, 3.15, 3.19 (all in the same file)
5. `src/backends/os_package_manager.rs` — Issue 3.14
6. `src/reboot.rs` — Issue 3.10 (signature change)
7. `src/ui/reboot_dialog.rs` — Issue 3.10 (call-site update)
8. `src/backends/flatpak.rs` — Issues 3.13 and 3.20 (both in the same file; do together)

---

## Validation Checklist

After implementation:
- `cargo build` must succeed with zero errors
- `cargo clippy -- -D warnings` must produce no warnings
- `cargo fmt --check` must pass
- `cargo test` must pass (existing tests in `upgrade.rs` must continue to pass)
- Manually verify: on a non-NixOS system inside Flatpak, Nix backend should not appear
- Manually verify: on NixOS inside Flatpak, Nix backend should appear correctly
- Manually verify: clicking refresh during an active update does not trigger a second check cycle
