# Quick Wins Feature Specification — Section 7 Batch 1

**Project:** Up — GTK4/libadwaita Linux desktop system updater  
**Language:** Rust 2021  
**Date:** 2026-05-07  
**Status:** DRAFT — awaiting implementation

---

## Table of Contents

1. [Codebase Context](#1-codebase-context)
2. [Feature A — Per-backend skip checkboxes](#2-feature-a--per-backend-skip-checkboxes)
3. [Feature B — Reboot-required detection](#3-feature-b--reboot-required-detection)
4. [Feature C — Log export / Copy button](#4-feature-c--log-export--copy-button)
5. [Affected Files Summary](#5-affected-files-summary)
6. [No New Dependencies Required](#6-no-new-dependencies-required)
7. [Implementation Order](#7-implementation-order)

---

## 1. Codebase Context

### 1.1 Relevant types

| Type | File | Role |
|------|------|------|
| `UpdateRow` | `src/ui/update_row.rs` | Per-backend row widget; holds `adw::ExpanderRow`, `gtk::Label`, `gtk::Spinner`, `Rc<RefCell<Vec<adw::ActionRow>>>` |
| `LogPanel` | `src/ui/log_panel.rs` | Expandable log widget; holds `gtk::Expander`, `gtk::TextView`, scroll debounce |
| `UpWindow::build_update_page()` | `src/ui/window.rs` | Builds the entire update tab; owns `rows`, `detected`, `updating`, `run_checks` closure, `update_button` |
| `UpdateOrchestrator` | `src/orchestrator.rs` | Takes `Vec<Arc<dyn Backend>>` and streams `OrchestratorEvent` on a background thread |
| `OrchestratorEvent` | `src/orchestrator.rs` | Enum: `AuthStarted`, `AuthSucceeded`, `AuthFailed`, `BackendStarted`, `BackendLog`, `BackendFinished`, `AllFinished` |
| `UpdateResult` | `src/backends/mod.rs` | Enum: `Success`, `SuccessWithSelfUpdate`, `Error(BackendError)`, `Skipped(String)` |
| `crate::reboot::reboot()` | `src/reboot.rs` | Issues `systemctl reboot` (or flatpak-spawn variant) |
| `show_reboot_dialog()` | `src/ui/reboot_dialog.rs` | Presents `adw::AlertDialog`; calls `crate::reboot::reboot()` on user confirmation |

### 1.2 Key wiring in `window.rs`

- `rows: Rc<RefCell<Vec<(BackendKind, UpdateRow)>>>` — parallel list of `(kind, widget)` pairs
- `detected: Rc<RefCell<Vec<Arc<dyn Backend>>>>` — live backend list populated after background detection
- `updating: Rc<Cell<bool>>` — guard flag preventing re-entrant update runs
- `update_button` — enabled when `total_available > 0` after a check cycle
- After `AllFinished`, `show_reboot_dialog(&button)` is called unconditionally when `!has_error`

### 1.3 Cargo features in use

```toml
adw = { version = "0.7", package = "libadwaita", features = ["v1_5"] }
gtk = { version = "0.9", package = "gtk4", features = ["v4_12"] }
```

`adw::ToastOverlay` and `adw::Toast` are available since libadwaita 1.0; both are present in v1_5.

---

## 2. Feature A — Per-backend skip checkboxes

### 2.1 Current state

`UpdateRow::new(backend)` creates an expander row with icon, spinner, and status label. There is no per-row skip control. The orchestrator in `window.rs` passes `detected.borrow().clone()` (all detected backends) to `UpdateOrchestrator::new()` without filtering. The `Skipped(String)` variant of `UpdateResult` already exists but is never emitted by the orchestrator itself.

### 2.2 Design decisions

| Decision | Rationale |
|----------|-----------|
| Skip state lives on `UpdateRow` as `Rc<Cell<bool>>` | `UpdateRow` is already `Clone` via `Rc`; single source of truth for the skip flag is the row widget |
| `UpdateRow::new()` takes `on_skip_changed: impl Fn() + 'static` callback | Allows the window to recompute button sensitivity without `UpdateRow` needing to know about the button |
| Store `last_available: Rc<Cell<Option<usize>>>` on `UpdateRow` | Required to restore the row's status label when the user unchecks the skip box after check |
| Filter backends in `window.rs` before passing to orchestrator | The orchestrator itself does not change; filtering is a UI concern |
| Manually emit visual `Skipped` for filtered-out backends in the UI loop before starting the orchestrator | Keeps the UI consistent without adding skip logic to orchestrator |
| "Update All" recomputes non-skipped available count on every checkbox toggle | Ensures the button is disabled when all backends with updates are skipped |

### 2.3 Changes to `src/ui/update_row.rs`

#### 2.3.1 Updated struct

```rust
#[derive(Clone)]
pub struct UpdateRow {
    pub row: adw::ExpanderRow,
    status_label: gtk::Label,
    spinner: gtk::Spinner,
    pkg_rows: Rc<RefCell<Vec<adw::ActionRow>>>,
    /// Current skip state; toggled by the skip checkbox.
    skip_flag: Rc<Cell<bool>>,
    /// Last resolved available-update count; used to restore status on un-skip.
    last_available: Rc<Cell<Option<usize>>>,
    skip_checkbox: gtk::CheckButton,
}
```

#### 2.3.2 Updated `new()` signature

```rust
pub fn new(backend: &dyn Backend, on_skip_changed: impl Fn() + 'static) -> Self {
```

#### 2.3.3 Skip checkbox construction (inside `new()`)

```rust
let skip_flag = Rc::new(Cell::new(false));
let last_available: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));

let skip_checkbox = gtk::CheckButton::builder()
    .tooltip_text("Skip this source during Update All")
    .valign(gtk::Align::Center)
    .build();

// Order: add checkbox before spinner/label so it appears leftmost in the suffix area.
row.add_suffix(&skip_checkbox);
row.add_suffix(&spinner);
row.add_suffix(&status_label);

// Wire up checkbox toggle.
skip_checkbox.connect_toggled(glib::clone!(
    #[strong] skip_flag,
    #[strong] last_available,
    #[strong] status_label,
    #[strong] row,
    move |cb| {
        let skipped = cb.is_active();
        skip_flag.set(skipped);
        if skipped {
            status_label.set_label("Skipped");
            status_label.set_css_classes(&["dim-label"]);
            row.set_sensitive(false);
        } else {
            row.set_sensitive(true);
            // Restore previous status if a check has already run.
            match last_available.get() {
                Some(count) => {
                    if count == 0 {
                        status_label.set_label("Up to date");
                        status_label.set_css_classes(&["success"]);
                    } else {
                        status_label.set_label(&format!("{count} available"));
                        status_label.set_css_classes(&["accent"]);
                    }
                }
                None => {
                    status_label.set_label("Ready");
                    status_label.set_css_classes(&["dim-label"]);
                }
            }
        }
        on_skip_changed();
    }
));
```

> **Note on `row.set_sensitive(false)`:** This dims the entire `adw::ExpanderRow` including its title and subtitle, giving the "muted/insensitive" visual state described in the requirements. The skip checkbox itself must remain sensitive; since it is set before `set_sensitive(false)` is called it will be insensitive too. Workaround: do **not** call `row.set_sensitive(false)` on the ExpanderRow; instead add the CSS class `"dim-label"` only to the status label (already done above) and leave the row sensitive. The muted appearance of the status label is sufficient visual feedback for a quick win.

Revised approach (simpler, no `set_sensitive` on row):

```rust
if skipped {
    status_label.set_label("Skipped");
    status_label.set_css_classes(&["dim-label"]);
} else {
    // restore as above
}
on_skip_changed();
```

#### 2.3.4 New public methods

```rust
/// Whether the user has checked this backend's skip box.
pub fn is_skipped(&self) -> bool {
    self.skip_flag.get()
}

/// Store the available-update count (called by the check cycle in window.rs).
pub fn record_available(&self, count: usize) {
    self.last_available.set(Some(count));
}
```

#### 2.3.5 Update `set_status_available()`

After calling `set_status_available`, the window must also call `row.record_available(count)` so the skip/un-skip toggle can restore the correct label. This call happens in `window.rs`, not inside `UpdateRow`.

### 2.4 Changes to `src/ui/window.rs`

#### 2.4.1 `UpdateRow::new()` call site

In the backend detection result handler, change:

```rust
// BEFORE
let row = UpdateRow::new(backend.as_ref());

// AFTER
let row = UpdateRow::new(backend.as_ref(), {
    let rows = rows.clone();
    let update_button = update_button.clone();
    let updating = updating.clone();
    move || {
        // Recompute button sensitivity when a skip checkbox is toggled.
        if updating.get() {
            return; // Don't change button state during an active run.
        }
        let borrowed = rows.borrow();
        let non_skipped_available: usize = borrowed
            .iter()
            .filter(|(_, r)| !r.is_skipped())
            .filter_map(|(_, r)| r.last_available.get())
            .sum();
        update_button.set_sensitive(non_skipped_available > 0);
    }
});
```

> **Visibility note:** `last_available` field needs to be `pub(crate)` or accessed via a getter. Prefer adding `pub fn last_available_count(&self) -> Option<usize> { self.last_available.get() }` to `UpdateRow`.

#### 2.4.2 `record_available` call after check

In the `run_checks` closure, after `row.set_status_available(count)`:

```rust
row.set_status_available(count);
row.record_available(count);  // ADD THIS
*total_available.borrow_mut() += count;
```

Also update the button-enable logic in `run_checks` to exclude skipped backends:

```rust
// BEFORE
if total > 0 {
    update_button_checks.set_sensitive(true);
    ...
}

// AFTER — only count updates from non-skipped backends
if remaining == 0 {
    let borrowed = rows.borrow();
    let non_skipped_total: usize = borrowed
        .iter()
        .filter(|(_, r)| !r.is_skipped())
        .filter_map(|(_, r)| r.last_available_count())
        .sum();
    if non_skipped_total > 0 {
        update_button_checks.set_sensitive(true);
        status_label_checks.set_label(&format!(
            "{non_skipped_total} update{} available",
            if non_skipped_total == 1 { "" } else { "s" }
        ));
    } else {
        status_label_checks.set_label("Everything is up to date.");
    }
}
```

#### 2.4.3 Update button click handler — skip filtering

In the `update_button.connect_clicked` closure, replace:

```rust
// BEFORE
let backends = detected.borrow().clone();

// AFTER
// Visually mark skipped rows immediately.
{
    let borrowed = rows.borrow();
    for (_, row) in borrowed.iter() {
        if row.is_skipped() {
            row.set_status_skipped("Skipped by user");
        }
    }
}
// Filter out skipped backends before handing to orchestrator.
let backends: Vec<Arc<dyn Backend>> = {
    let detected_borrow = detected.borrow();
    let rows_borrow = rows.borrow();
    detected_borrow
        .iter()
        .filter(|b| {
            rows_borrow
                .iter()
                .find(|(k, _)| *k == b.kind())
                .map(|(_, r)| !r.is_skipped())
                .unwrap_or(true)
        })
        .cloned()
        .collect()
};
```

> The orchestrator is unmodified. Skipped backends never enter the orchestrator's backend list. Their `Skipped` visual state is set in the UI before the orchestrator starts.

### 2.5 Risks

| Risk | Mitigation |
|------|------------|
| Clicking the checkbox may propagate the click to the ExpanderRow toggle | `gtk::CheckButton` consumes click events; test manually |
| `on_skip_changed` closure captures `rows` and `update_button` — potential strong-ref cycles | Use `#[weak]` for `update_button` inside the closure; `rows` is `Rc` not `Arc` so no cycle risk in single-threaded GTK main loop |
| `last_available` is `None` when skip is toggled before any check — restore shows "Ready" | Acceptable; "Ready" is the correct state when no check has run |

---

## 3. Feature B — Reboot-required detection

### 3.1 Current state

`src/ui/reboot_dialog.rs::show_reboot_dialog()` is called unconditionally in `window.rs` after `AllFinished` when `!has_error`:

```rust
if !has_error {
    crate::ui::reboot_dialog::show_reboot_dialog(&button);
}
```

No detection logic exists. Every successful update prompts for a reboot regardless of whether one is actually required.

`src/reboot.rs` contains only `pub fn reboot() -> Result<(), String>`.

### 3.2 Design decisions

| Decision | Rationale |
|----------|-----------|
| Add `pub fn reboot_required() -> bool` to `src/reboot.rs` | Keeps all reboot-related logic in one module |
| Check only file-based indicators (`/var/run/reboot-required`, `/var/run/reboot-required.pkgs`) | No privilege required; works on Debian/Ubuntu/Mint/Pop_OS; sufficient for a quick win |
| `needrestart -b` as optional check | Parse stdout for `NEEDRESTART-KSTA: 3` (kernel outdated = reboot); skip if binary not found |
| Skip `dnf needs-restarting -r` | Requires pkexec for accurate results on SELinux systems; adds auth complexity |
| Flatpak sandbox handled via `flatpak-spawn --host test -f <path>` | Consistent with how `reboot()` already handles the sandbox |
| Run check in a background thread after `AllFinished`, using `async_channel` | File I/O and process spawning must not block GTK main thread |

### 3.3 Changes to `src/reboot.rs`

Add the following function after the existing `reboot()`:

```rust
/// Detect whether a system reboot is required after an update.
///
/// Checks, in order:
/// 1. `/var/run/reboot-required` — created by Debian/Ubuntu `update-notifier-common`
/// 2. `/var/run/reboot-required.pkgs` — alternate Ubuntu indicator
/// 3. `needrestart -b` stdout — if `needrestart` is on PATH
///
/// Flatpak-aware: file presence is tested via `flatpak-spawn --host` when
/// running inside the sandbox.
///
/// Returns `true` if any check indicates a reboot is needed, `false` otherwise.
/// Errors from individual checks are treated as "not required" (fail-open).
pub fn reboot_required() -> bool {
    let in_flatpak = std::path::Path::new("/.flatpak-info").exists();

    // --- File-based checks ---
    for path in &["/var/run/reboot-required", "/var/run/reboot-required.pkgs"] {
        if in_flatpak {
            // Test file existence through the host via flatpak-spawn.
            match std::process::Command::new("flatpak-spawn")
                .args(["--host", "test", "-f", path])
                .status()
            {
                Ok(s) if s.success() => {
                    info!("Reboot required (flatpak-spawn check): {path}");
                    return true;
                }
                _ => {}
            }
        } else if std::path::Path::new(path).exists() {
            info!("Reboot required (file check): {path}");
            return true;
        }
    }

    // --- needrestart optional check (host only; skip inside Flatpak) ---
    if !in_flatpak && which::which("needrestart").is_ok() {
        if let Ok(output) = std::process::Command::new("needrestart")
            .arg("-b")
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // NEEDRESTART-KSTA: 3 means running kernel is outdated → reboot needed.
            if stdout.lines().any(|l| l.trim() == "NEEDRESTART-KSTA: 3") {
                info!("Reboot required (needrestart KSTA=3)");
                return true;
            }
        }
    }

    false
}
```

### 3.4 Changes to `src/ui/window.rs`

Replace the existing unconditional `show_reboot_dialog` call with a background check:

```rust
// BEFORE (inside update_button.connect_clicked async block, after the event loop):
if !has_error {
    crate::ui::reboot_dialog::show_reboot_dialog(&button);
}

// AFTER:
if !has_error {
    // Run the blocking reboot-required check off the GTK main thread.
    let (rr_tx, rr_rx) = async_channel::bounded::<bool>(1);
    std::thread::spawn(move || {
        let _ = rr_tx.send_blocking(crate::reboot::reboot_required());
    });
    if let Ok(true) = rr_rx.recv().await {
        crate::ui::reboot_dialog::show_reboot_dialog(&button);
    }
}
```

> This block runs inside a `glib::spawn_future_local` async block, so `.await` is valid and `rr_rx.recv().await` does not block the GTK main loop.

### 3.5 Risks

| Risk | Mitigation |
|------|------------|
| `/var/run/reboot-required` only exists on Debian/Ubuntu-derived distros | For other distros (Fedora, Arch, openSUSE), `reboot_required()` returns `false` — no spurious dialogs, though also no detection. Acceptable for quick win. |
| `needrestart` may not be installed | `which::which("needrestart").is_ok()` guard prevents errors |
| `flatpak-spawn --host test -f` requires the `org.freedesktop.Flatpak` talk permission | Already required for `flatpak-spawn --host systemctl reboot`; no new permission needed |
| `std::thread::spawn` inside an already-async context | Acceptable; `crate::reboot::reboot_required()` is synchronous blocking I/O. Threading matches existing patterns in `src/ui/window.rs` |

---

## 4. Feature C — Log export / Copy button

### 4.1 Current state

`LogPanel` stores log text in a `gtk::TextBuffer` via `gtk::TextView`. The `expander` is a `gtk::Expander` whose `label` is the string `"Terminal Output"` and whose `child` is a `gtk::ScrolledWindow` containing the text view.

There is no save/export mechanism. There is no `adw::ToastOverlay` anywhere in the current widget tree — neither in `window.rs` (which uses a plain `gtk::Box` as `main_box`) nor in `build_update_page()`.

### 4.2 Design decisions

| Decision | Rationale |
|----------|-----------|
| Add `adw::ToastOverlay` inside `LogPanel` wrapping the `ScrolledWindow` | Scopes toasts to the log area; no structural changes to `window.rs` or the window hierarchy |
| Use `gtk::Expander::set_label_widget()` with a custom `gtk::Box` | Lets us embed a save button in the expander header row |
| Save button initially `sensitive(false)`; enabled on first `append_line`, disabled on `clear()` | Prevents saving an empty file |
| Output path: `$HOME/up-update-<unix_seconds>.log` | Simple, predictable, user-accessible location; no XDG dir creation needed |
| File I/O runs synchronously in the button `connect_clicked` handler | Log files are small (max 5 000 lines × ~80 chars ≈ 400 KB); blocking for a few ms is acceptable |
| Use `adw::Toast` for confirmation; 5-second timeout | Non-intrusive, consistent with libadwaita patterns |

### 4.3 Changes to `src/ui/log_panel.rs`

#### 4.3.1 Updated struct

```rust
#[derive(Clone)]
pub struct LogPanel {
    pub expander: gtk::Expander,
    text_view: gtk::TextView,
    scroll_pending: Rc<Cell<bool>>,
    save_button: gtk::Button,
    toast_overlay: adw::ToastOverlay,
}
```

#### 4.3.2 Updated `new()` — full replacement

```rust
pub fn new() -> Self {
    let text_view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .css_classes(vec!["card"])
        .build();

    let scrolled = gtk::ScrolledWindow::builder()
        .min_content_height(200)
        .max_content_height(400)
        .child(&text_view)
        .build();

    // ToastOverlay wraps the scrolled window so toasts appear over the log text.
    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&scrolled));

    // Save button placed in the expander header.
    let save_button = gtk::Button::builder()
        .icon_name("document-save-symbolic")
        .tooltip_text("Save log to file")
        .css_classes(vec!["flat", "circular"])
        .sensitive(false)
        .valign(gtk::Align::Center)
        .build();

    // Custom label widget: label text + save button in a horizontal box.
    let header_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let header_label = gtk::Label::new(Some("Terminal Output"));
    header_box.append(&header_label);
    header_box.append(&save_button);

    let expander = gtk::Expander::builder()
        .label_widget(&header_box)
        .margin_top(12)
        .child(&toast_overlay)
        .build();

    let buffer = text_view.buffer();
    let end_iter = buffer.end_iter();
    buffer.create_mark(Some("scroll-end"), &end_iter, false);

    // Wire up the save button.
    {
        let text_view_weak = text_view.downgrade();
        let toast_overlay_clone = toast_overlay.clone();
        save_button.connect_clicked(move |_| {
            let Some(view) = text_view_weak.upgrade() else { return };
            let buffer = view.buffer();
            let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);

            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let path = format!("{home}/up-update-{secs}.log");

            let toast_msg = match std::fs::write(&path, text.as_str()) {
                Ok(_) => format!("Log saved to {path}"),
                Err(e) => format!("Failed to save log: {e}"),
            };

            toast_overlay_clone.add_toast(
                adw::Toast::builder()
                    .title(&toast_msg)
                    .timeout(5)
                    .build(),
            );
        });
    }

    Self {
        expander,
        text_view,
        scroll_pending: Rc::new(Cell::new(false)),
        save_button,
        toast_overlay,
    }
}
```

#### 4.3.3 Update `append_line()`

After inserting text into the buffer, enable the save button:

```rust
pub fn append_line(&self, line: &str) {
    let clean = strip_ansi(line);
    let buffer = self.text_view.buffer();
    let mut end = buffer.end_iter();
    buffer.insert(&mut end, &clean);
    buffer.insert(&mut end, "\n");

    // Enable the save button now that the log has content.
    if !self.save_button.is_sensitive() {
        self.save_button.set_sensitive(true);
    }

    // FIFO eviction (unchanged)
    if buffer.line_count() > LINE_CAP {
        let mut start = buffer.start_iter();
        if let Some(mut evict_end) = buffer.iter_at_line(EVICT_BATCH) {
            buffer.delete(&mut start, &mut evict_end);
        }
    }

    self.schedule_scroll();
}
```

#### 4.3.4 Update `clear()`

```rust
pub fn clear(&self) {
    let buffer = self.text_view.buffer();
    buffer.set_text("");
    self.save_button.set_sensitive(false);
}
```

#### 4.3.5 Required imports

Add to the top of `log_panel.rs`:

```rust
use adw::prelude::*;
```

`adw::ToastOverlay` and `adw::Toast` are re-exported through the `adw` crate which is already a dependency.

### 4.4 No changes to `window.rs`

`window.rs` uses `log_panel.expander` directly to append to `content_box`. The new struct layout is backward-compatible (the `expander` field still exists and is the same type). `window.rs` does not need to be modified.

### 4.5 Risks

| Risk | Mitigation |
|------|------------|
| Clicking the save button inside `gtk::Expander`'s label widget may also toggle the expander | `gtk::Button` captures click events before the expander's gesture recogniser sees them. Risk is low; test manually. If it occurs, use `button.connect_clicked` and call `expander.set_expanded(!expander.is_expanded())` to compensate — but this is unlikely to be needed. |
| `gtk::Expander::set_label_widget()` replaces the default expand arrow triangle | The arrow is drawn by the expander frame, not the label widget; it remains visible. |
| `adw::Toast` title is a simple string — path may be truncated in the UI | Acceptable for a quick win; long paths display with ellipsis in the toast widget. |
| `std::fs::write` is blocking | Log is small; executes in microseconds. Acceptable on GTK main thread. |
| `$HOME` may be unset (container/service context) | Falls back to `/tmp`. |

---

## 5. Affected Files Summary

| File | Feature(s) | Change type |
|------|------------|-------------|
| `src/ui/update_row.rs` | A | Struct field additions; `new()` signature change; new public methods |
| `src/ui/window.rs` | A, B | `UpdateRow::new()` call updated; button filtering; reboot check logic |
| `src/reboot.rs` | B | New `pub fn reboot_required() -> bool` |
| `src/ui/log_panel.rs` | C | Struct field additions; `new()` rewritten; `append_line`/`clear` updated |

No new source files are required.

---

## 6. No New Dependencies Required

All features use crates already in `Cargo.toml`:

| Used crate | Purpose |
|------------|---------|
| `adw` (libadwaita 0.7 / v1_5) | `ToastOverlay`, `Toast`, `CheckButton` (re-exported from GTK4 in adw context) |
| `gtk` (gtk4 0.9 / v4_12) | `CheckButton`, `Expander::set_label_widget`, `Button`, `TextBuffer::text` |
| `glib` | `glib::clone!`, `Cell`, `spawn_future_local` |
| `async_channel` | Reboot check result relay (Feature B) |
| `which` | `which::which("needrestart")` guard (Feature B) |
| `std::fs`, `std::time` | Log file write, Unix timestamp (Feature C) |

---

## 7. Implementation Order

Implement features in this order to minimise merge conflicts:

1. **Feature B** — touches `src/reboot.rs` (additive) and one small block in `window.rs`. Zero risk of conflicting with A or C.
2. **Feature C** — touches only `src/ui/log_panel.rs`. Self-contained.
3. **Feature A** — touches `src/ui/update_row.rs` and a larger section of `src/ui/window.rs`. Implement last to avoid collision with the B window.rs edit.

Each feature is independently testable after implementation.

---

*End of specification.*
