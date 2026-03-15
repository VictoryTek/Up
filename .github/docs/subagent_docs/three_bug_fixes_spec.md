# Three Bug Fixes Specification

## Current State Analysis

### File Inventory

**`data/io.github.up.desktop`** — Desktop entry file. Sets `Icon=io.github.up` (correct XDG convention — no extension, no path). Defines the app as `Type=Application` in `Categories=System;PackageManager;`.

**`meson.build`** — Build system configuration. Installs the desktop file, metainfo XML, SVG icon at `scalable/apps/`, and conditionally installs PNG icons at 256x256, 128x128, and 48x48 using `fs.exists()` checks. Does **not** import the `gnome` meson module and does **not** call `gnome.post_install()` for icon cache/desktop database updates.

**`data/icons/hicolor/`** — Icon directory structure:
- `scalable/apps/io.github.up.svg` — EXISTS ✓
- `256x256/apps/io.github.up.png` — EXISTS ✓
- `128x128/apps/` — EMPTY (no PNG) ✗
- `48x48/apps/` — EMPTY (no PNG) ✗

**`src/main.rs`** — Entry point. Initializes `env_logger`, creates `UpApplication`, calls `run()`.

**`src/app.rs`** — Application struct wrapping `adw::Application`. On activate, creates `UpWindow` and presents it.

**`src/ui/mod.rs`** — Module declarations for `window`, `update_row`, `log_panel`, `upgrade_page`.

**`src/ui/window.rs`** — Main window with `adw::ViewStack` containing Update and Upgrade pages. The `build_update_page()` method detects backends, creates `UpdateRow` widgets, and wires up the "Update All" button. The button handler spawns a background thread running all backends sequentially via Tokio, streams log output and results through `async_channel` back to the GTK main loop.

**`src/ui/update_row.rs`** — Individual backend row widget (`adw::ActionRow`) with status label and spinner. Has methods: `set_status_running()`, `set_status_success()`, `set_status_error()`, `set_status_skipped()`. Note: `set_status_running()` is **never called** from anywhere.

**`src/ui/log_panel.rs`** — Expandable terminal output panel using `gtk::Expander` with a `gtk::TextView`.

**`src/runner.rs`** — `CommandRunner` struct that runs system commands via `tokio::process::Command`, streaming stdout/stderr line-by-line through an `async_channel::Sender`.

**`src/backends/mod.rs`** — Defines `BackendKind` enum, `UpdateResult` enum, `Backend` trait (with `run_update`), and `detect_backends()` function.

**`src/ui/upgrade_page.rs`** — Upgrade page UI. Detects distro info, shows system information, has "Run Checks" and "Start Upgrade" buttons. The "Upgrade Available" row subtitle is set to `"Checking..."` if `upgrade_supported` is true, but **no async task is ever spawned** to resolve this check.

**`src/upgrade.rs`** — Distro detection via `/etc/os-release`, prerequisite checks (packages, disk space, backup), and upgrade execution functions for Ubuntu, Fedora, openSUSE, and NixOS.

---

## Problem Definition

### Bug 1: PNG icon not used as .desktop icon

The `.desktop` file sets `Icon=io.github.up`, which follows XDG conventions correctly. The actual problems are:

1. **Missing `gnome.post_install()`** in `meson.build`. Without this call, the post-install scripts that run `gtk-update-icon-cache`, `update-desktop-database`, and `glib-compile-schemas` are never executed. The icon theme cache is not updated after installation, so desktop environments may not find the installed icon.

2. **Missing PNG icons at 128x128 and 48x48.** The directories exist but contain no files. While `meson.build` handles this gracefully with `fs.exists()`, many desktop environments and app launchers prefer these common sizes. Having only a 256x256 PNG and an SVG means some environments may show a generic icon.

### Bug 2: Update progress bar and "Updating" status never completes

**Root Cause: Channel sender not dropped, causing deadlock.**

In `window.rs` `build_update_page()`, the update button click handler:

```rust
let (result_tx, result_rx) = async_channel::unbounded::<(BackendKind, UpdateResult)>();
let result_tx_clone = result_tx.clone();
```

- `result_tx_clone` is moved into `std::thread::spawn` and dropped after backends complete.
- `result_tx` (the original) remains alive in the outer `glib::spawn_future_local` closure scope.
- The receiving loop `while let Ok((kind, result)) = result_rx.recv().await` will **never** see the channel close because `result_tx` is still alive.
- Therefore `status_ref.set_label("Update complete.")` is **never reached**.

The same issue exists for the log channel (`tx`/`tx_clone`), but that loop runs in a separate spawned future so it doesn't directly block completion — it just leaks.

**Secondary issue:** `UpdateRow::set_status_running()` is defined but never called. Individual rows never show "Updating..." while their backend is running — they stay on "Ready" until results arrive.

**Missing feature:** No progress bar exists to show overall update progress.

### Bug 3: Upgrade tab "Checking..." never resolves

**Root Cause: No async check is ever performed.**

In `upgrade_page.rs`, the "Upgrade Available" row is built with:

```rust
.subtitle(if distro_info.upgrade_supported {
    "Checking..."
} else {
    "Not supported for this distribution yet"
})
```

The subtitle is set to `"Checking..."` but **no code ever updates it**. No async task is spawned to check for upgrade availability. The string is static and remains "Checking..." forever.

---

## Proposed Solution Architecture

### Bug 1 Fix: Icon installation

1. **Add `gnome.post_install()` to `meson.build`** to update icon cache and desktop database on install.
2. **Generate missing PNG icons** at 128x128 and 48x48 from the existing 256x256 PNG or scalable SVG using ImageMagick/rsvg-convert in a script, OR add pre-generated PNGs to the repo.

Since the 128x128 and 48x48 PNGs need to be actual image files, and we cannot generate them in the spec, the implementation agent should generate them from the existing assets. However, the simplest approach is to note that the SVG at scalable/ is the authoritative source and `gnome.post_install()` will handle icon cache updates. The missing PNGs are a nice-to-have but not the root cause.

**Primary fix:** Add `gnome.post_install()` to `meson.build`.

### Bug 2 Fix: Channel deadlock + progress bar + "Updating" status

1. **Drop the original senders** (`tx` and `result_tx`) before entering the receiving loops.
2. **Call `set_status_running()`** on each row before starting updates, and set individual rows to "Updating..." as each backend begins.
3. **Add a `gtk::ProgressBar`** (or `adw::Clamp`-wrapped `gtk::ProgressBar`) to show overall progress. Calculate progress as `completed_backends / total_backends`.

### Bug 3 Fix: Upgrade availability check

1. **Add `check_upgrade_available()` function** to `src/upgrade.rs` that checks if a newer version is available for the detected distro.
2. **Spawn an async task** in `upgrade_page.rs` on page build that runs the check on a background thread and updates the "Upgrade Available" row subtitle with the result.

---

## Implementation Steps

### File 1: `meson.build`

**Change:** Import the `gnome` module and call `gnome.post_install()`.

Add after the `project()` declaration (around line 7):

```meson
gnome = import('gnome')
```

Add at the very end of the file (after the `foreach` loop):

```meson
gnome.post_install(
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
```

This ensures `gtk-update-icon-cache` and `update-desktop-database` are run after `meson install`, so the icon theme cache includes our icons and the desktop entry is registered.

---

### File 2: `src/ui/window.rs`

**Change 1: Add progress bar widget.**

In `build_update_page()`, after the `status_label` creation (around line 105) and before the `backends_group` creation, add a progress bar:

```rust
// Progress bar
let progress_bar = gtk::ProgressBar::builder()
    .visible(false)
    .show_text(true)
    .build();
content_box.append(&progress_bar);
```

**Change 2: Fix channel deadlock — drop original senders before receiving.**

Inside the `update_button.connect_clicked` closure, in the `glib::spawn_future_local` block, after the `std::thread::spawn(...)` call and **before** the log/result processing loops:

Add explicit drops of the original senders:

```rust
// Drop original senders so channels close when the thread finishes
drop(tx);
drop(result_tx);
```

The closure variables need to be restructured so that `tx` and `result_tx` are available to drop. Currently `tx_clone` is what's passed to the thread. The fix:

Restructure the channel sender usage:
- Create `(tx, rx)` 
- Clone `tx` for the thread: `let tx_thread = tx.clone();`
- Drop the original `tx` before the receive loop
- Same for `result_tx`

**The corrected click handler body (full replacement):**

```rust
update_button.connect_clicked(move |button| {
    button.set_sensitive(false);
    status_clone.set_label("Updating...");
    progress_clone.set_visible(true);
    progress_clone.set_fraction(0.0);
    progress_clone.set_text(Some("Starting..."));
    log_clone.clear();

    // Set all rows to "Updating..." state
    {
        let rows_borrowed = rows_clone.borrow();
        for (_, row) in rows_borrowed.iter() {
            row.set_status_running();
        }
    }

    let rows_ref = rows_clone.clone();
    let log_ref = log_clone.clone();
    let status_ref = status_clone.clone();
    let progress_ref = progress_clone.clone();
    let button_ref = button.clone();
    let backends = detected_clone.clone();
    let total_backends = backends.len() as f64;

    glib::spawn_future_local(async move {
        let (tx, rx) = async_channel::unbounded::<(BackendKind, String)>();
        let (result_tx, result_rx) =
            async_channel::unbounded::<(BackendKind, UpdateResult)>();

        // Clone senders for the worker thread
        let tx_thread = tx.clone();
        let result_tx_thread = result_tx.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            rt.block_on(async {
                for backend in &backends {
                    let kind = backend.kind();
                    let runner = CommandRunner::new(tx_thread.clone(), kind);
                    let result = backend.run_update(&runner).await;
                    let _ = result_tx_thread.send((kind, result)).await;
                }
            });

            drop(tx_thread);
            drop(result_tx_thread);
        });

        // Drop the original senders so channels close when the thread finishes
        drop(tx);
        drop(result_tx);

        // Process log output in a separate future
        let log_ref2 = log_ref.clone();
        glib::spawn_future_local(async move {
            while let Ok((kind, line)) = rx.recv().await {
                log_ref2.append_line(&format!("[{kind}] {line}"));
            }
        });

        // Process results
        let mut completed: f64 = 0.0;
        let mut has_error = false;
        while let Ok((kind, result)) = result_rx.recv().await {
            completed += 1.0;
            let fraction = completed / total_backends;
            progress_ref.set_fraction(fraction);
            progress_ref.set_text(Some(&format!(
                "{}/{} complete",
                completed as usize,
                total_backends as usize
            )));

            let rows_borrowed = rows_ref.borrow();
            if let Some((_, row)) = rows_borrowed.iter().find(|(k, _)| *k == kind) {
                match &result {
                    UpdateResult::Success { updated_count } => {
                        row.set_status_success(*updated_count);
                    }
                    UpdateResult::Error(msg) => {
                        row.set_status_error(msg);
                        has_error = true;
                    }
                    UpdateResult::Skipped(msg) => {
                        row.set_status_skipped(msg);
                    }
                }
            }
        }

        if has_error {
            status_ref.set_label("Update completed with errors.");
        } else {
            status_ref.set_label("Update complete.");
        }
        progress_ref.set_fraction(1.0);
        progress_ref.set_text(Some("Done"));
        button_ref.set_sensitive(true);
    });
});
```

**Variable capture changes:** The closure needs to also capture `progress_clone` (a clone of `progress_bar`). Add these clones alongside the existing ones before the `connect_clicked`:

```rust
let progress_clone = progress_bar.clone();
```

---

### File 3: `src/ui/upgrade_page.rs`

**Change: Spawn async upgrade availability check on page build.**

After the `upgrade_available_row` is created and added to `info_group`, if `distro_info.upgrade_supported` is true, spawn an async task to check for the upgrade:

```rust
if distro_info.upgrade_supported {
    let upgrade_row_clone = upgrade_available_row.clone();
    let distro_check = distro_info.clone();

    glib::spawn_future_local(async move {
        let (tx, rx) = async_channel::unbounded::<String>();

        std::thread::spawn(move || {
            let result = check_upgrade_available(&distro_check);
            let _ = tx.send_blocking(result);
            drop(tx);
        });

        if let Ok(result_msg) = rx.recv().await {
            upgrade_row_clone.set_subtitle(&result_msg);
        } else {
            upgrade_row_clone.set_subtitle("Could not determine upgrade availability");
        }
    });
}
```

This requires importing `check_upgrade_available` from `crate::upgrade`, which we need to add.

---

### File 4: `src/upgrade.rs`

**Change: Add `check_upgrade_available()` function.**

Add a new public function that checks if an upgrade is available for the detected distro:

```rust
/// Check if a distribution upgrade is available.
pub fn check_upgrade_available(distro: &DistroInfo) -> String {
    match distro.id.as_str() {
        "ubuntu" => check_ubuntu_upgrade(),
        "fedora" => check_fedora_upgrade(&distro.version_id),
        "debian" => check_debian_upgrade(),
        "opensuse-leap" => check_opensuse_upgrade(),
        "nixos" => check_nixos_upgrade(&distro.version_id),
        _ => "Not supported for this distribution".to_string(),
    }
}

fn check_ubuntu_upgrade() -> String {
    // Check if do-release-upgrade reports a new version
    match Command::new("do-release-upgrade")
        .args(["-c"])
        .output()
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("New release") || stdout.contains("new release") {
                let line = stdout.lines()
                    .find(|l| l.contains("New release") || l.contains("new release"))
                    .unwrap_or("New release available");
                format!("Yes — {}", line.trim())
            } else {
                "No upgrade available".to_string()
            }
        }
        Err(_) => "Could not check (do-release-upgrade not found)".to_string(),
    }
}

fn check_fedora_upgrade(current_version_id: &str) -> String {
    let current: u32 = current_version_id.parse().unwrap_or(0);
    let next = current + 1;
    // Check if the next Fedora release exists by querying the release URL
    match Command::new("curl")
        .args([
            "-s", "-o", "/dev/null", "-w", "%{http_code}",
            &format!("https://dl.fedoraproject.org/pub/fedora/linux/releases/{}/Everything/x86_64/os/", next),
        ])
        .output()
    {
        Ok(output) => {
            let code = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if code == "200" || code == "301" || code == "302" {
                format!("Yes — Fedora {} is available", next)
            } else {
                format!("No — Fedora {} not yet released", next)
            }
        }
        Err(_) => "Could not check (curl not found)".to_string(),
    }
}

fn check_debian_upgrade() -> String {
    // For Debian, check if the current codename has a successor
    "Check manually at https://www.debian.org/releases/".to_string()
}

fn check_opensuse_upgrade() -> String {
    "Check manually at https://get.opensuse.org/leap/".to_string()
}

fn check_nixos_upgrade(current_version_id: &str) -> String {
    // NixOS versions are like "24.11". Check if a newer channel exists.
    // Parse current version, compute next
    let parts: Vec<&str> = current_version_id.split('.').collect();
    if parts.len() == 2 {
        if let (Ok(year), Ok(month)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
            let (next_year, next_month) = if month >= 11 {
                (year + 1, 5)
            } else {
                (year, 11)
            };
            let next_channel = format!("nixos-{}.{:02}", next_year, next_month);
            // Check if the channel URL exists
            match Command::new("curl")
                .args([
                    "-s", "-o", "/dev/null", "-w", "%{http_code}",
                    &format!("https://channels.nixos.org/{}", next_channel),
                ])
                .output()
            {
                Ok(output) => {
                    let code = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if code == "200" || code == "301" || code == "302" {
                        format!("Yes — NixOS {}.{:02} is available", next_year, next_month)
                    } else {
                        format!("No — NixOS {}.{:02} not yet available", next_year, next_month)
                    }
                }
                Err(_) => "Could not check (curl not found)".to_string(),
            }
        } else {
            "Could not parse current NixOS version".to_string()
        }
    } else {
        "Could not parse current NixOS version".to_string()
    }
}
```

---

## Detailed Change Summary

### `meson.build`
| Line | Change |
|------|--------|
| After line 7 | Add `gnome = import('gnome')` |
| End of file | Add `gnome.post_install(gtk_update_icon_cache: true, update_desktop_database: true)` |

### `src/ui/window.rs`
| Location | Change |
|----------|--------|
| After `status_label` append (line ~107) | Add `gtk::ProgressBar` creation and append to `content_box` |
| Before `update_button.connect_clicked` clones (line ~127) | Add `let progress_clone = progress_bar.clone();` |
| Inside `connect_clicked` closure | 1. Show progress bar, set fraction to 0.0 <br> 2. Call `set_status_running()` on all rows <br> 3. Rename `tx_clone`→`tx_thread`, `result_tx_clone`→`result_tx_thread` <br> 4. Add `drop(tx); drop(result_tx);` after `std::thread::spawn` block <br> 5. Track `completed` count, update progress bar on each result <br> 6. Differentiate "complete" vs "completed with errors" status |

### `src/ui/upgrade_page.rs`
| Location | Change |
|----------|--------|
| After `upgrade_available_row` is added to `info_group` | Spawn `glib::spawn_future_local` that runs `check_upgrade_available()` on a background thread, updates the row subtitle with the result |
| Imports | Add `use crate::upgrade::check_upgrade_available;` |

### `src/upgrade.rs`
| Location | Change |
|----------|--------|
| After existing functions | Add `pub fn check_upgrade_available(distro: &DistroInfo) -> String` and helper functions for each supported distro |

---

## Dependencies

No new crate dependencies are needed. All required crates are already in `Cargo.toml`:
- `gtk4` (v0.9) — provides `gtk::ProgressBar`
- `async-channel` (v2) — already used for channel communication
- `glib` (v0.20) — already used for `glib::spawn_future_local`
- `serde` / `serde_json` — already used for serialization

The `gnome` meson module is a built-in meson module, no additional system dependencies needed.

---

## Risks and Mitigations

### Risk 1: `gnome.post_install()` requires meson ≥ 0.59
**Mitigation:** This is a very old meson version (2021). Any modern system will have it. The project already uses `import('fs')` which also requires meson ≥ 0.53.

### Risk 2: Upgrade availability checks shell out to `curl`
**Mitigation:** The checks are best-effort. If `curl` is not available, a descriptive fallback message is returned ("Could not check"). The checks run on a background thread and won't block the UI even if they time out.

### Risk 3: Progress bar accuracy
**Mitigation:** Progress is measured as `completed_backends / total_backends`, which provides coarse but honest progress. Each backend is one unit of work. This is simple and predictable.

### Risk 4: Race condition on `set_status_running()` call
**Mitigation:** `set_status_running()` is called on the GTK main thread (inside the `connect_clicked` closure, before spawning the async future or background thread), so it's safe. All UI updates happen on the main thread via `glib::spawn_future_local`.

### Risk 5: Channel drop ordering
**Mitigation:** The fix explicitly drops `tx` and `result_tx` after spawning the thread but before entering the receive loops. The thread holds clones (`tx_thread`, `result_tx_thread`), which it drops when done. This guarantees the channels close properly when the thread completes.
