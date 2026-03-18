# Reboot Prompt Specification

**Feature:** Show a "Reboot Now / Later" dialog after successful updates or upgrades  
**Project:** Up — GTK4/libadwaita Linux desktop application (Rust)  
**Date:** 2026-03-18  

---

## 1. Current State Analysis

### 1.1 Update Flow — `src/ui/window.rs` (`build_update_page`)

The update flow runs inside a `glib::spawn_future_local` block:

1. A worker `std::thread` runs all detected backends sequentially.
2. A log channel `(tx, rx): async_channel::unbounded::<(BackendKind, String)>` streams output to the `LogPanel`.
3. A results channel `(result_tx, result_rx): async_channel::unbounded::<(BackendKind, UpdateResult)>` carries per-backend outcomes.
4. After all results are received, a `has_error: bool` flag is evaluated:
   - `has_error == false` → `status_label` set to `"Update complete."`
   - `has_error == true`  → `status_label` set to `"Update completed with errors."`
5. `button_ref.set_sensitive(true)` is called unconditionally.
6. **No reboot prompt is shown.**

**Success condition is already tracked** via `has_error`. Only `UpdateResult::Error` sets this flag; `Success` and `Skipped` do not.

### 1.2 Upgrade Flow — `src/ui/upgrade_page.rs` (`UpgradePage::build`)

The upgrade flow runs inside a `glib::spawn_future_local` block:

1. An `adw::AlertDialog` is used for initial confirmation (the pattern we will reuse).
2. After confirmation, a worker thread calls `upgrade::execute_upgrade(&distro, &tx)`.
3. `execute_upgrade` is `fn execute_upgrade(distro: &DistroInfo, tx: &async_channel::Sender<String>)` — currently returns `()`.
4. Individual commands are run via `run_streaming_command`, which checks exit codes but does not surface success/failure to callers.
5. After the log channel drains, `button_ref2.set_sensitive(true)` is called.
6. **No success/failure tracking exists; no reboot prompt is shown.**

### 1.3 Existing `adw::AlertDialog` Usage

`upgrade_page.rs` already uses the exact pattern needed:

```rust
let dialog = adw::AlertDialog::builder()
    .heading("Confirm System Upgrade")
    .body(format!(...))
    .build();
dialog.add_response("cancel", "Cancel");
dialog.add_response("upgrade", "Upgrade");
dialog.set_response_appearance("upgrade", adw::ResponseAppearance::Destructive);
dialog.set_default_response(Some("cancel"));
dialog.set_close_response("cancel");
dialog.connect_response(None, move |_dialog, response| {
    if response == "upgrade" { /* ... */ }
});
dialog.present(Some(&widget));
```

`adw::AlertDialog` is available — the project already enables the `v1_5` libadwaita feature flag (`adw = { version = "0.7", features = ["v1_5"] }`), which is required for `AdwAlertDialog`.

### 1.4 `Cargo.toml` Dependencies

```toml
gtk  = { version = "0.9", package = "gtk4",      features = ["v4_12"] }
adw  = { version = "0.7", package = "libadwaita", features = ["v1_5"]  }
glib = "0.20"
```

No new crate dependencies are required. The reboot command uses `std::process::Command` (stdlib).

---

## 2. Problem Definition

After a successful update or upgrade, users must manually reboot their system for kernel or library changes to take effect. The application currently provides no guidance or convenience for this. The desired behaviour:

- Show a non-blocking, optional reboot prompt **only on success**.
- If the user chooses "Reboot Now", issue a `systemctl reboot` command.
- If the user chooses "Later", dismiss the dialog and do nothing.
- Work correctly both natively and inside a Flatpak sandbox.

---

## 3. Proposed Solution Architecture

### 3.1 New Files

| File | Purpose |
|---|---|
| `src/reboot.rs` | Flatpak detection + `trigger_reboot()` utility |
| `src/ui/reboot_dialog.rs` | `show_reboot_prompt(parent)` UI helper |

### 3.2 Modified Files

| File | Change |
|---|---|
| `src/main.rs` | Add `mod reboot;` |
| `src/ui/mod.rs` | Add `pub mod reboot_dialog;` |
| `src/upgrade.rs` | `run_streaming_command` → `bool`, `upgrade_*` sub-functions → `bool`, `execute_upgrade` → `bool` |
| `src/ui/upgrade_page.rs` | Add result channel; detect success; call `show_reboot_prompt` on success |
| `src/ui/window.rs` | Call `show_reboot_prompt` when `!has_error` after update completes |

### 3.3 Reboot Execution Strategy

| Environment | Command |
|---|---|
| Native (non-Flatpak) | `systemctl reboot` |
| Flatpak sandbox | `flatpak-spawn --host systemctl reboot` |

**Detection:** Check whether `/.flatpak-info` exists at runtime. This file is created by the Flatpak runtime in every sandboxed app instance and is the canonical detection method used by GTK apps (e.g., GNOME Software, GNOME Settings).

### 3.4 Data Flow Diagram

```
Update flow (window.rs):
  [Update All clicked]
    → worker thread runs backends
    → result_rx drains in GTK loop
    → has_error determined
    → if !has_error: show_reboot_prompt(&button_ref)

Upgrade flow (upgrade_page.rs):
  [Start Upgrade confirmed]
    → worker thread runs execute_upgrade() → returns bool
    → result_tx sends bool to result_rx
    → log channel drains in GTK loop
    → result_rx.recv().await → success: bool
    → if success: show_reboot_prompt(&button_ref2)
```

---

## 4. Implementation Steps

### Step 1 — Create `src/reboot.rs`

```rust
use std::path::Path;
use std::process::Command;

/// Returns true when the app is running inside a Flatpak sandbox.
/// The `/.flatpak-info` file is injected by the Flatpak runtime and is the
/// canonical way to detect Flatpak at runtime (used by GNOME platform apps).
pub fn is_flatpak() -> bool {
    Path::new("/.flatpak-info").exists()
}

/// Issue a system reboot.
/// Inside Flatpak, tunnels through `flatpak-spawn --host` to reach the host systemd.
/// Outside Flatpak, calls `systemctl reboot` directly.
/// Uses `Command::spawn` (fire-and-forget) so the GTK loop is not blocked.
pub fn trigger_reboot() {
    if is_flatpak() {
        Command::new("flatpak-spawn")
            .args(["--host", "systemctl", "reboot"])
            .spawn()
            .ok();
    } else {
        Command::new("systemctl")
            .arg("reboot")
            .spawn()
            .ok();
    }
}
```

### Step 2 — Create `src/ui/reboot_dialog.rs`

```rust
use adw::prelude::*;
use gtk::glib;

/// Present a "Reboot Now / Later" dialog attached to `parent`.
/// Only calls `crate::reboot::trigger_reboot()` if the user chooses "reboot".
/// Follows the same `adw::AlertDialog` pattern used in `upgrade_page.rs`.
pub fn show_reboot_prompt(parent: &impl gtk::prelude::IsA<gtk::Widget>) {
    let dialog = adw::AlertDialog::builder()
        .heading("Reboot Recommended")
        .body(
            "Updates have been applied. A reboot is recommended \
             to activate the latest changes.",
        )
        .build();

    dialog.add_response("later", "Later");
    dialog.add_response("reboot", "Reboot Now");
    dialog.set_response_appearance("reboot", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("later"));
    dialog.set_close_response("later");

    dialog.connect_response(None, move |_dialog, response| {
        if response == "reboot" {
            crate::reboot::trigger_reboot();
        }
    });

    dialog.present(Some(parent));
}
```

**Notes on the dialog design:**
- "Later" is the default (selected by Enter/close) — non-intrusive; the user is never forced to reboot.
- "Reboot Now" uses `ResponseAppearance::Suggested` (blue) — positive, not destructive.
- `dialog.present(Some(parent))` accepts any `IsA<gtk::Widget>` — passing the button that triggered the flow is sufficient (libadwaita walks up to the parent window).

### Step 3 — Register `reboot` module in `src/main.rs`

Add `mod reboot;` alongside the existing module declarations:

```rust
mod app;
mod backends;
mod reboot;       // ← add this line
mod runner;
mod ui;
mod upgrade;
```

### Step 4 — Register `reboot_dialog` module in `src/ui/mod.rs`

```rust
pub mod log_panel;
pub mod reboot_dialog;   // ← add this line
pub mod update_row;
pub mod upgrade_page;
pub mod window;
```

### Step 5 — Modify `src/upgrade.rs`: surface success/failure

#### 5a. `run_streaming_command` returns `bool`

Change the signature and return values:

```rust
// Before:
fn run_streaming_command(program: &str, args: &[&str], tx: &async_channel::Sender<String>) {

// After:
fn run_streaming_command(program: &str, args: &[&str], tx: &async_channel::Sender<String>) -> bool {
```

At the end of the `match result { Ok(mut child) => { ... } }` block, return the correct boolean:

```rust
match result {
    Ok(mut child) => {
        // ... stdout/stderr streaming unchanged ...
        match child.wait() {
            Ok(status) => {
                if status.success() {
                    let _ = tx.send_blocking("Command completed successfully.".into());
                    true   // ← was no return value
                } else {
                    let code = status.code().unwrap_or(-1);
                    let _ = tx.send_blocking(format!("Command exited with code {code}"));
                    false  // ← was no return value
                }
            }
            Err(e) => {
                let _ = tx.send_blocking(format!("Failed to wait for process: {e}"));
                false
            }
        }
    }
    Err(e) => {
        let _ = tx.send_blocking(format!("Failed to start {program}: {e}"));
        false
    }
}
```

#### 5b. Upgrade sub-functions return `bool`

Each distro-specific function changes from `fn upgrade_X(tx: ...) { }` to `fn upgrade_X(tx: ...) -> bool { }` and returns the `&&`-combined result of its `run_streaming_command` calls.

**`upgrade_ubuntu`:**
```rust
fn upgrade_ubuntu(tx: &async_channel::Sender<String>) -> bool {
    let _ = tx.send_blocking("Running: do-release-upgrade -f DistUpgradeViewNonInteractive".into());
    run_streaming_command(
        "pkexec",
        &["do-release-upgrade", "-f", "DistUpgradeViewNonInteractive"],
        tx,
    )
}
```

**`upgrade_fedora`:**
```rust
fn upgrade_fedora(tx: &async_channel::Sender<String>) -> bool {
    let _ = tx.send_blocking("Installing system-upgrade plugin...".into());
    let ok1 = run_streaming_command(
        "pkexec",
        &["dnf", "install", "-y", "dnf-plugin-system-upgrade"],
        tx,
    );

    let _ = tx.send_blocking("Downloading upgrade packages...".into());
    let next_version = detect_next_fedora_version();
    let ver_str = next_version.to_string();
    let ok2 = run_streaming_command(
        "pkexec",
        &["dnf", "system-upgrade", "download", "--releasever", &ver_str, "-y"],
        tx,
    );

    let _ = tx.send_blocking("Download complete. The system will reboot to apply the upgrade.".into());
    let ok3 = run_streaming_command("pkexec", &["dnf", "system-upgrade", "reboot"], tx);

    ok1 && ok2 && ok3
}
```

**`upgrade_opensuse`:**
```rust
fn upgrade_opensuse(tx: &async_channel::Sender<String>) -> bool {
    let _ = tx.send_blocking("Running zypper distribution upgrade...".into());
    run_streaming_command("pkexec", &["zypper", "dup", "-y"], tx)
}
```

**`upgrade_nixos`:**
```rust
fn upgrade_nixos(tx: &async_channel::Sender<String>) -> bool {
    let config_type = detect_nixos_config_type();
    match config_type {
        NixOsConfigType::LegacyChannel => {
            let _ = tx.send_blocking("Detected: legacy channel-based NixOS config".into());
            let _ = tx.send_blocking("Updating NixOS channel...".into());
            let ok1 = run_streaming_command("sudo", &["nix-channel", "--update"], tx);
            let _ = tx.send_blocking("Rebuilding NixOS (switch --upgrade)...".into());
            let ok2 = run_streaming_command("pkexec", &["nixos-rebuild", "switch", "--upgrade"], tx);
            ok1 && ok2
        }
        NixOsConfigType::Flake => {
            let _ = tx.send_blocking("Detected: flake-based NixOS config".into());
            let _ = tx.send_blocking("Updating flake inputs in /etc/nixos...".into());
            let ok1 = run_streaming_command(
                "sudo",
                &["nix", "flake", "update", "--flake", "/etc/nixos"],
                tx,
            );
            let hostname = detect_hostname();
            let flake_target = format!("/etc/nixos#{}", hostname);
            let _ = tx.send_blocking(format!("Rebuilding NixOS configuration: {flake_target}"));
            let ok2 = run_streaming_command(
                "pkexec",
                &["nixos-rebuild", "switch", "--flake", &flake_target],
                tx,
            );
            ok1 && ok2
        }
    }
}
```

#### 5c. `execute_upgrade` returns `bool`

```rust
// Before:
pub fn execute_upgrade(distro: &DistroInfo, tx: &async_channel::Sender<String>) {

// After:
pub fn execute_upgrade(distro: &DistroInfo, tx: &async_channel::Sender<String>) -> bool {
```

```rust
pub fn execute_upgrade(distro: &DistroInfo, tx: &async_channel::Sender<String>) -> bool {
    let _ = tx.send_blocking(format!(
        "Starting upgrade for {} {}...",
        distro.name, distro.version
    ));

    match distro.id.as_str() {
        "ubuntu" | "debian" => upgrade_ubuntu(tx),
        "fedora" => upgrade_fedora(tx),
        "opensuse-leap" => upgrade_opensuse(tx),
        "nixos" => upgrade_nixos(tx),
        _ => {
            let _ = tx.send_blocking(format!(
                "Upgrade is not yet supported for '{}'. Supported: Ubuntu, Fedora, openSUSE Leap, NixOS.",
                distro.name
            ));
            false
        }
    }
}
```

### Step 6 — Modify `src/ui/upgrade_page.rs`: detect success, show reboot dialog

In the `upgrade_button.connect_clicked` handler, inside the `if response == "upgrade"` branch, change the `glib::spawn_future_local` block:

**Before:**
```rust
glib::spawn_future_local(async move {
    let (tx, rx) = async_channel::unbounded::<String>();
    let tx_clone = tx.clone();

    std::thread::spawn(move || {
        upgrade::execute_upgrade(&distro2, &tx_clone);
        drop(tx_clone);
    });

    drop(tx);

    while let Ok(line) = rx.recv().await {
        log_ref2.append_line(&line);
    }

    button_ref2.set_sensitive(true);
});
```

**After:**
```rust
glib::spawn_future_local(async move {
    let (tx, rx) = async_channel::unbounded::<String>();
    let (result_tx, result_rx) = async_channel::bounded::<bool>(1);
    let tx_clone = tx.clone();
    let result_tx_clone = result_tx.clone();

    std::thread::spawn(move || {
        let success = upgrade::execute_upgrade(&distro2, &tx_clone);
        let _ = result_tx_clone.send_blocking(success);
        drop(tx_clone);
        drop(result_tx_clone);
    });

    drop(tx);
    drop(result_tx);

    while let Ok(line) = rx.recv().await {
        log_ref2.append_line(&line);
    }

    let success = result_rx.recv().await.unwrap_or(false);
    button_ref2.set_sensitive(true);

    if success {
        crate::ui::reboot_dialog::show_reboot_prompt(&button_ref2);
    }
});
```

### Step 7 — Modify `src/ui/window.rs`: show reboot dialog after successful update

In `build_update_page()`, inside the `glib::spawn_future_local` block, change the final section after the results loop:

**Before:**
```rust
                if has_error {
                    status_ref.set_label("Update completed with errors.");
                } else {
                    status_ref.set_label("Update complete.");
                }
                button_ref.set_sensitive(true);
```

**After:**
```rust
                if has_error {
                    status_ref.set_label("Update completed with errors.");
                } else {
                    status_ref.set_label("Update complete.");
                    crate::ui::reboot_dialog::show_reboot_prompt(&button_ref);
                }
                button_ref.set_sensitive(true);
```

The `button_ref` variable is already in scope at that point and is a `gtk::Button`, which implements `IsA<gtk::Widget>` — exactly what `show_reboot_prompt` requires.

---

## 5. GTK4 / libadwaita API Reference

All API calls used are from libadwaita ≥ 1.5, which is already enabled via the `v1_5` feature flag.

| Call | Purpose |
|---|---|
| `adw::AlertDialog::builder().heading(...).body(...).build()` | Construct the dialog |
| `dialog.add_response("later", "Later")` | Add "Later" button |
| `dialog.add_response("reboot", "Reboot Now")` | Add "Reboot Now" button |
| `dialog.set_response_appearance("reboot", adw::ResponseAppearance::Suggested)` | Style "Reboot Now" as blue/suggested |
| `dialog.set_default_response(Some("later"))` | Enter key activates "Later" (safe default) |
| `dialog.set_close_response("later")` | Escape/swipe-close activates "Later" |
| `dialog.connect_response(None, move \|_d, response\| { ... })` | Handle response asynchronously in GTK loop |
| `dialog.present(Some(&widget))` | Present modally; widget can be any widget in the window |

The `connect_response` signal fires on the GTK main thread, so calling `std::process::Command::spawn()` inside it is safe (non-blocking fire-and-forget).

---

## 6. Flatpak Sandbox Considerations

### Detection
`/.flatpak-info` is a file injected by the Flatpak runtime into every sandboxed application's filesystem namespace. It is the canonical detection mechanism used by GNOME platform applications (GNOME Software, GNOME Settings, etc.).

### Reboot inside Flatpak
Direct use of `systemctl` from within a Flatpak sandbox is blocked by the sandbox policy. The correct escape hatch is `flatpak-spawn --host <command>`, which tunnels the command through the Flatpak portal to the host system. This requires the app to have the `org.freedesktop.Flatpak` D-Bus talk permission or the `--talk-name=org.freedesktop.Flatpak` Flatpak manifest permission.

**Manifest consideration:** If the app is distributed as a Flatpak and needs `flatpak-spawn --host`, add the following to `io.github.up.json`:
```json
"finish-args": [
  "--talk-name=org.freedesktop.Flatpak"
]
```

This is already commonly done by GNOME apps that need host-system access (e.g., `io.github.flatseal`).

### Alternative: D-Bus `org.freedesktop.login1`
The D-Bus interface `org.freedesktop.login1.Manager.Reboot` can also trigger a reboot from within a Flatpak. However, this requires adding `gio` D-Bus calls and the `--system-talk-name=org.freedesktop.login1` permission. The `flatpak-spawn --host systemctl reboot` approach is simpler, uses no extra crates, and is the standard pattern for `Up`'s existing command architecture. **We use `flatpak-spawn` for simplicity and consistency.**

---

## 7. Risks and Mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| User accidentally clicks "Reboot Now" during work | Medium | "Later" is the default response (activated by Enter and Escape). "Reboot Now" requires an explicit click. |
| `systemctl reboot` fails (non-systemd distro, e.g., Alpine, Void with runit) | Low | `Command::spawn().ok()` silently ignores errors. The system simply won't reboot; no crash or panic. |
| Flatpak without `org.freedesktop.Flatpak` permission | Low | `flatpak-spawn` will fail silently (`.ok()`). Add the permission to the manifest if Flatpak distribution is added in future. |
| Upgrade flow: Fedora's `dnf system-upgrade reboot` already reboots the system | Medium | The Fedora upgrade step intentionally reboots into the offline upgrade environment. The in-app reboot prompt will never be reached for Fedora because the system reboots before `execute_upgrade` returns. This is correct behaviour — no defensive code needed. |
| `execute_upgrade` returns `false` for unsupported distros | Low (only affects unknown IDs) | Already handled — the function returns `false` for the wildcard `_` arm, so the prompt is never shown. |
| Concurrent updates causing spurious prompt | None | Both update and upgrade flows disable their trigger button before starting, preventing double-execution. |

---

## 8. Source Research References

1. **libadwaita `AdwAlertDialog` documentation** — GNOME developer docs.  
   Confirms `AdwAlertDialog` is the correct modern dialog widget (introduced libadwaita 1.5); `connect_response` callback pattern; `present(parent)` accepting any widget.  
   Source: `https://gnome.pages.gitlab.gnome.org/libadwaita/doc/1-latest/class.AlertDialog.html`

2. **libadwaita migration guide: Adaptive Dialogs** — GNOME GitLab.  
   Confirms `adw_dialog_present(dialog, parent)` where parent can be any widget in the window tree; `can-close`, `close-attempt`, `default-response`, `close-response` API.  
   Source: `https://gnome.pages.gitlab.gnome.org/libadwaita/doc/1-latest/migrating-to-adaptive-dialogs.html`

3. **`flatpak-spawn` man page / Flatpak developer docs** — Flatpak project.  
   Confirms `flatpak-spawn --host <cmd>` as the standard way to run host commands from a sandboxed Flatpak app; requires `org.freedesktop.Flatpak` bus name.  
   Source: `https://docs.flatpak.org/en/latest/sandbox-permissions-reference.html`

4. **`/.flatpak-info` detection** — Flatpak developer documentation.  
   Confirms `/.flatpak-info` as the canonical Flatpak runtime detection file; present in all Flatpak apps since Flatpak 0.9.0.  
   Source: `https://docs.flatpak.org/en/latest/introduction.html`

5. **`org.freedesktop.login1` D-Bus interface** — freedesktop.org / systemd documentation.  
   Documents `Manager.Reboot(interactive: bool)` method; used as alternative to `systemctl reboot` for sandboxed apps. Evaluated but not used — `flatpak-spawn` is simpler.  
   Source: `https://www.freedesktop.org/software/systemd/man/latest/org.freedesktop.login1.html`

6. **gtk4-rs / libadwaita-rs Rust bindings** — docs.rs.  
   Confirms Rust API equivalents: `adw::AlertDialog::builder()`, `.add_response()`, `.set_response_appearance()`, `.connect_response()`, `.present()`; `adw::ResponseAppearance::Suggested`.  
   Source: `https://gtk-rs.org/gtk4-rs/stable/latest/docs/libadwaita/struct.AlertDialog.html`

7. **GNOME HIG: Dialogs** — GNOME Human Interface Guidelines.  
   Confirms UX best practices: confirmation dialogs should be non-modal when possible; destructive actions should be confirmed; safe defaults should be highlighted; "cancel/postpone" should always be available.  
   Source: `https://developer.gnome.org/hig/patterns/feedback/dialogs.html`

8. **std::process::Command — Rust standard library docs** — doc.rust-lang.org.  
   Confirms `Command::spawn()` is non-blocking; returns `Result<Child>` immediately; suitable for fire-and-forget processes from the GTK main thread.  
   Source: `https://doc.rust-lang.org/std/process/struct.Command.html`

---

## 9. Summary of All Files to Create or Modify

### New Files
- `src/reboot.rs`
- `src/ui/reboot_dialog.rs`

### Modified Files
- `src/main.rs` — add `mod reboot;`
- `src/ui/mod.rs` — add `pub mod reboot_dialog;`
- `src/upgrade.rs` — `run_streaming_command` → `bool`, sub-functions → `bool`, `execute_upgrade` → `bool`
- `src/ui/upgrade_page.rs` — add result channel, show prompt on success
- `src/ui/window.rs` — show prompt when `!has_error` after update completes

---

## 10. Out of Scope

- Persisting a "reboot pending" state across app restarts.
- Detecting whether a reboot is *actually required* (e.g., checking `/var/run/reboot-required` on Debian/Ubuntu) — the prompt is shown on any successful update.
- Adding a system tray notification for pending reboots.

These may be considered as follow-on features.
