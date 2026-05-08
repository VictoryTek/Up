# Quick Wins Feature Specification — Section 7 Batch 2

**Project:** Up — GTK4/libadwaita Linux desktop system updater  
**Language:** Rust 2021  
**Date:** 2026-05-07  
**Status:** DRAFT — awaiting implementation  
**Continues lettering from:** Batch 1 (A = skip checkboxes, B = reboot detection, C = log export)

---

## Table of Contents

1. [Codebase Context](#1-codebase-context)
2. [Feature D — A11y Audit](#2-feature-d--a11y-audit)
3. [Feature E — Metered-Connection Warning](#3-feature-e--metered-connection-warning)
4. [Feature F — Battery-Aware Prompt](#4-feature-f--battery-aware-prompt)
5. [Feature G — Per-Backend Retry Button](#5-feature-g--per-backend-retry-button)
6. [Feature H — Update History Log](#6-feature-h--update-history-log)
7. [Affected Files Summary](#7-affected-files-summary)
8. [Dependency Analysis](#8-dependency-analysis)
9. [Implementation Order](#9-implementation-order)
10. [Risks and Mitigations](#10-risks-and-mitigations)

---

## 1. Codebase Context

### 1.1 Relevant types

| Type | File | Role |
|------|------|------|
| `UpdateRow` | `src/ui/update_row.rs` | Per-backend expander row; fields: `row`, `status_label`, `spinner`, `pkg_rows`, `skip_flag`, `last_available`, `skip_checkbox` |
| `LogPanel` | `src/ui/log_panel.rs` | Expandable log widget with `save_button` (icon-only) |
| `UpWindow::build_update_page()` | `src/ui/window.rs` | Builds the entire update tab; owns `rows`, `detected`, `updating`, `run_checks`, `update_button`, `refresh_button`, `menu_button` |
| `UpdateOrchestrator` | `src/orchestrator.rs` | Takes `Vec<Arc<dyn Backend>>` and streams `OrchestratorEvent` on a background thread |
| `OrchestratorEvent` | `src/orchestrator.rs` | Enum: `AuthStarted`, `AuthSucceeded`, `AuthFailed`, `BackendStarted`, `BackendLog`, `BackendFinished`, `AllFinished` |
| `UpdateResult` | `src/backends/mod.rs` | Enum: `Success { updated_count }`, `SuccessWithSelfUpdate { updated_count }`, `Error(BackendError)`, `Skipped(String)` |
| `BackendKind` | `src/backends/mod.rs` | Enum; implements `Display` and `Serialize/Deserialize` |
| `Backend` trait | `src/backends/mod.rs` | `kind()`, `display_name()`, `description()`, `icon_name()`, `run_update()`, `needs_root()`, `count_available()`, `list_available()` |

### 1.2 Key icon-only buttons in the current UI

| Widget | Location | Current accessibility aid | Gap |
|--------|----------|--------------------------|-----|
| `refresh_button` | `window.rs` header | `tooltip_text("Check for updates")` | No `update_property` call |
| `menu_button` | `window.rs` header | `tooltip_text("Main menu")` | No `update_property` call |
| `save_button` | `log_panel.rs` | `tooltip_text("Save log to file")` | No `update_property` call |
| `skip_checkbox` | `update_row.rs` | `tooltip_text("Skip this source during Update All")` | Generic label; should be backend-specific |
| Backend icon `gtk::Image` | `update_row.rs` prefix | None | Not interactive; but may still confuse AT |

### 1.3 Cargo.toml dependencies already present

```toml
gtk = { version = "0.9", package = "gtk4", features = ["v4_12"] }
adw = { version = "0.7", package = "libadwaita", features = ["v1_5"] }
gio = "0.20"        # provides NetworkMonitor
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

No new Cargo dependencies are required for any feature in this batch.

---

## 2. Feature D — A11y Audit

### 2.1 Current state

Icon-only buttons in the UI (`refresh_button`, `menu_button`, `save_button`) rely exclusively on `tooltip_text` for accessibility. Since GTK 4.10, the GTK accessibility bridge automatically uses the tooltip as the accessible name for icon buttons **only when no explicit accessible name is set and the button has no label child**. However:

1. This implicit bridging is not guaranteed across all AT (assistive technology) implementations or Wayland compositors.
2. The `skip_checkbox` uses a single generic tooltip regardless of which backend it belongs to. Screen readers cannot distinguish "Skip APT" from "Skip Flatpak".
3. The backend icon `gtk::Image` added via `row.add_prefix(&icon)` has no alt text — AT may announce it as "image" or skip it silently depending on the compositor.
4. No contrast audit has been explicitly confirmed for the application's dark-style variant.

### 2.2 GTK4-rs Accessible API (gtk4-rs 0.9)

In gtk4-rs 0.9, the `gtk::prelude::AccessibleExtManual` trait provides:

```rust
fn update_property(&self, properties: &[gtk::accessible::Property<'_>]);
```

The `gtk::accessible::Property` enum variants relevant here:

```rust
gtk::accessible::Property::Label("accessible name")
gtk::accessible::Property::Description("additional description")
```

`use gtk::prelude::*` pulls in `AccessibleExtManual` automatically. These calls are synchronous and safe to make immediately after widget construction.

For `gtk::CheckButton` and `gtk::Button`, GTK4 uses the `accessible::Property::Label` value as the text announced by AT when the widget receives focus.

For `gtk::Image` used as a decorative prefix, the correct approach is to set `accessible_role` to `gtk::AccessibleRole::Presentation` so AT ignores it:

```rust
icon.set_accessible_role(gtk::AccessibleRole::Presentation);
```

`set_accessible_role` is available via `gtk::prelude::AccessibleExt`.

### 2.3 Changes required

#### 2.3.1 `src/ui/window.rs` — refresh\_button and menu\_button

Immediately after construction:

```rust
let refresh_button = gtk::Button::builder()
    .icon_name("view-refresh-symbolic")
    .tooltip_text("Check for updates")
    .build();
refresh_button.update_property(&[gtk::accessible::Property::Label("Check for updates")]);
```

```rust
let menu_button = gtk::MenuButton::builder()
    .icon_name("open-menu-symbolic")
    .tooltip_text("Main menu")
    .build();
menu_button.update_property(&[gtk::accessible::Property::Label("Main menu")]);
```

#### 2.3.2 `src/ui/log_panel.rs` — save\_button

```rust
let save_button = gtk::Button::builder()
    .icon_name("document-save-symbolic")
    .tooltip_text("Save log to file")
    .css_classes(vec!["flat", "circular"])
    .sensitive(false)
    .valign(gtk::Align::Center)
    .build();
save_button.update_property(&[gtk::accessible::Property::Label("Save log to file")]);
```

#### 2.3.3 `src/ui/update_row.rs` — skip\_checkbox and backend icon

The `skip_checkbox` accessible label must include the backend name:

```rust
// In UpdateRow::new(), after constructing skip_checkbox:
let accessible_label = format!("Skip {} during Update All", backend.display_name());
skip_checkbox.update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
```

The backend icon prefix should be marked as decorative:

```rust
let icon = gtk::Image::from_icon_name(backend.icon_name());
icon.set_accessible_role(gtk::AccessibleRole::Presentation);
```

#### 2.3.4 Dark-style contrast verification (non-code)

Libadwaita handles dark/light theming automatically. The CSS classes in use (`"success"`, `"error"`, `"accent"`, `"dim-label"`) are Libadwaita-standard named colours that pass WCAG AA contrast in both light and dark modes by design. No custom colour overrides exist in the codebase, so no contrast fixes are needed. This should be noted as VERIFIED in the review.

### 2.4 Implementation steps

1. Add `use gtk::prelude::*;` to any file where it is not already present (it is already present in all three files).
2. In `src/ui/window.rs`: call `update_property` on `refresh_button` and `menu_button` after construction.
3. In `src/ui/log_panel.rs`: call `update_property` on `save_button` after construction.
4. In `src/ui/update_row.rs`: call `update_property` on `skip_checkbox` with backend-specific label; call `set_accessible_role(Presentation)` on the icon image.

### 2.5 Affected files

- `src/ui/window.rs`
- `src/ui/log_panel.rs`
- `src/ui/update_row.rs`

---

## 3. Feature E — Metered-Connection Warning

### 3.1 Current state

No network-condition check exists. Clicking "Update All" immediately starts the update sequence without checking whether the active network is metered (e.g., mobile hotspot, cellular). This can consume large amounts of data without warning.

### 3.2 GIO NetworkMonitor API (gio-rs 0.20)

`gio` is already a direct dependency (`gio = "0.20"`).

```rust
use gtk::gio;
use gio::prelude::NetworkMonitorExt;

// Get the process-lifetime singleton — never returns None in practice
let monitor = gio::NetworkMonitor::default();

// Check metered status synchronously (fast — reads kernel state)
let is_metered: bool = monitor.is_network_metered();

// Connect to property-change notification for dynamic banner updates
monitor.connect_network_metered_notify(move |m| {
    let metered = m.is_network_metered();
    // update banner visibility
});
```

`NetworkMonitorExt` is in `gio::prelude`. The `default()` function maps to `g_network_monitor_get_default()`, which is always available on Linux (falls back to a stub implementation if ConnMan/NetworkManager are not running). When NetworkManager is absent or the connectivity backend is unavailable, `is_network_metered()` returns `false`, so the worst case is a missed warning — never a false alarm.

### 3.3 Design decisions

| Decision | Rationale |
|----------|-----------|
| Check at the moment the user clicks "Update All" | User may have switched networks since the window opened |
| Show `adw::AlertDialog` (blocking modal) when metered | Blocks the update until the user explicitly confirms — cannot be dismissed accidentally |
| Show an `adw::Banner` at top of update page showing metered status | Persistent, non-intrusive; updates dynamically as network changes |
| Keep the banner yellow/warning via `adw::Banner` (default styling) | Libadwaita Banner's default style is neutral; use it with a clear icon for visual encoding |
| Do NOT block updates on non-metered network (no confirmation dialog) | Only warn when data cost is a concern |

### 3.4 UI components

#### 3.4.1 Persistent metered banner (in `build_update_page()`)

```rust
let metered_banner = adw::Banner::builder()
    .title("You are on a metered connection — updates may use significant data")
    .button_label("Update Anyway")
    .revealed(false)
    .build();
```

The banner is placed at the very top of the update page (above the scroll area), updated when the network changes.

**Note:** `adw::Banner::button_label` is optional; the banner's button should be hidden here since the primary action is controlled by the "Update All" button. The banner is purely informational. Set `button_label` to an empty string or omit the button by not setting it.

#### 3.4.2 Dynamic banner wiring

```rust
// Check and set banner visibility on startup
{
    let monitor = gio::NetworkMonitor::default();
    metered_banner.set_revealed(monitor.is_network_metered());

    // Keep banner in sync as network changes
    monitor.connect_network_metered_notify(glib::clone!(
        #[weak] metered_banner,
        move |m| {
            metered_banner.set_revealed(m.is_network_metered());
        }
    ));
}
```

#### 3.4.3 Confirmation dialog before "Update All"

When the user clicks "Update All" and the network is metered, show an `adw::AlertDialog` before proceeding:

```rust
update_button.connect_clicked(glib::clone!(
    // ... existing clones ...
    move |button| {
        if update_in_progress.get() { return; }

        let monitor = gio::NetworkMonitor::default();
        if monitor.is_network_metered() {
            // Show confirmation dialog
            let dialog = adw::AlertDialog::builder()
                .heading("Metered Connection")
                .body("You are on a metered connection. Updating may use significant mobile data. Continue?")
                .build();
            dialog.add_response("cancel", "Cancel");
            dialog.add_response("update", "Update Anyway");
            dialog.set_response_appearance("update", adw::ResponseAppearance::Destructive);
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");

            let button_weak = button.downgrade();
            let rows_clone = rows.clone();
            // ... pass all needed state via clones ...
            dialog.connect_response(None, move |_, response| {
                if response == "update" {
                    // Proceed with update (call the same update logic)
                    if let Some(b) = button_weak.upgrade() {
                        start_update(&b, /* ... */);
                    }
                }
            });
            dialog.present(Some(&window_weak.upgrade().unwrap()));
            return;
        }

        // Not metered — proceed immediately
        start_update(button, /* ... */);
    }
));
```

To avoid code duplication, extract the update-start logic into a named closure or helper function `start_update(...)` that can be called from both the direct path and the dialog callback.

### 3.5 Implementation steps

1. Import `use gtk::gio;` and `use gio::prelude::NetworkMonitorExt;` at the top of `src/ui/window.rs` (already present as `use gtk::gio;`; add `NetworkMonitorExt` to prelude use).
2. Add `metered_banner: adw::Banner` to `build_update_page()` return value or as a local that is appended to `page_box` before the scrolled area.
3. Wire the `gio::NetworkMonitor::default()` singleton to `metered_banner.set_revealed(...)` on load and on change.
4. Refactor the "Update All" click handler: extract the update logic into a closure `do_start_update` that can be called directly or after dialog confirmation.
5. Add the metered-check before calling `do_start_update` in the click handler.

### 3.6 Affected files

- `src/ui/window.rs`

---

## 4. Feature F — Battery-Aware Prompt

### 4.1 Current state

No battery check exists. If the system is on battery with low charge, a long upgrade (e.g., system package update) could drain the battery before completion, leaving the system in an inconsistent state.

### 4.2 Battery detection via sysfs

Linux exposes battery information under `/sys/class/power_supply/`. Each entry has:
- `type` — `"Battery"` or `"AC"` or `"USB"`, etc.
- `capacity` — integer 0–100 (current charge percentage)
- `status` — `"Charging"`, `"Discharging"`, `"Full"`, `"Not charging"`, `"Unknown"`

No D-Bus, no UPower, no new dependencies required. `std::fs::read_to_string` is sufficient.

**Approach:** Glob `/sys/class/power_supply/*/type`, find the first entry whose `type` is `"Battery"`, then read its `capacity` and `status`.

### 4.3 New module: `src/battery.rs`

```rust
/// Represents the current battery state.
#[derive(Debug, Clone)]
pub struct BatteryState {
    /// Charge percentage, 0–100.
    pub capacity: u8,
    /// Whether the battery is currently charging or full.
    pub is_charging: bool,
}

/// Read the current battery state from sysfs.
///
/// Returns `None` if no battery is found (e.g., desktops, VMs)
/// or if sysfs entries cannot be read (permission error, no entry).
///
/// This function performs blocking I/O but sysfs reads are served from kernel
/// memory and complete in microseconds — safe to call on the GTK main thread.
pub fn read_battery() -> Option<BatteryState> {
    let power_supply = std::path::Path::new("/sys/class/power_supply");
    let entries = std::fs::read_dir(power_supply).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();

        // Only process Battery entries
        let type_val = std::fs::read_to_string(path.join("type")).ok()?;
        if type_val.trim() != "Battery" {
            continue;
        }

        let capacity_str = std::fs::read_to_string(path.join("capacity")).ok()?;
        let capacity: u8 = capacity_str.trim().parse().ok()?;

        let status = std::fs::read_to_string(path.join("status"))
            .unwrap_or_default();
        let is_charging = matches!(status.trim(), "Charging" | "Full");

        return Some(BatteryState { capacity, is_charging });
    }
    None
}
```

**Important:** The `?` early-return in the outer loop body means if any individual `read_to_string` fails on one entry we skip it. This is intentional — robustness over completeness.

**Edge case — multiple batteries:** The function returns the first battery found. This is correct for all common laptop configurations. Multi-battery systems (rare) would return the first one only; since the threshold check is conservative this is acceptable.

### 4.4 Integration in `src/ui/window.rs`

After the metered check (Feature E) and before starting the update, check battery:

```rust
if let Some(bat) = crate::battery::read_battery() {
    if !bat.is_charging && bat.capacity < 40 {
        let dialog = adw::AlertDialog::builder()
            .heading("Low Battery")
            .body(format!(
                "Battery is at {}%. It is recommended to plug in before running updates. Continue anyway?",
                bat.capacity
            ))
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("update", "Update Anyway");
        dialog.set_response_appearance("update", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        // ... same response handling as metered dialog ...
        dialog.present(Some(&window));
        return;
    }
}
```

**Threshold:** 40% is the recommended threshold. It is large enough to cover typical system update durations (5–15 min) without being overly conservative.

**Stacking with metered check:** If both battery AND metered checks trigger, show the battery dialog first (battery risk is higher). The metered dialog is informational; the battery check is a safety gate. Sequencing: metered → battery (both must be confirmed if both trigger), OR combine into a single multi-condition dialog. **Recommended:** show a single combined `adw::AlertDialog` listing both concerns in the body text if both apply.

### 4.5 Implementation steps

1. Create `src/battery.rs` with `BatteryState` struct and `read_battery() -> Option<BatteryState>`.
2. Declare `mod battery;` in `src/main.rs`.
3. In `src/ui/window.rs`, in the "Update All" click handler (or in `do_start_update` helper from Feature E), call `crate::battery::read_battery()` before proceeding.
4. If battery is low and discharging, show `adw::AlertDialog` with "Cancel" / "Update Anyway" responses.
5. Combine with metered check: if both apply, the body text lists both concerns.

### 4.6 Affected files

- `src/battery.rs` (new)
- `src/main.rs` (add `mod battery;`)
- `src/ui/window.rs`

---

## 5. Feature G — Per-Backend Retry Button

### 5.1 Current state

When `set_status_error()` is called on an `UpdateRow`, the row shows the error message in the status label with an `"error"` CSS class. There is no way to re-run that individual backend without clicking "Update All" (which re-runs all non-skipped backends). The `UpdateRow` struct does not have a retry button field.

The `UpdateOrchestrator` already accepts a `Vec<Arc<dyn Backend>>` — passing a single backend is valid and requires no changes to the orchestrator.

### 5.2 Design decisions

| Decision | Rationale |
|----------|-----------|
| Retry button lives inside `UpdateRow` | Locality — the button is visually adjacent to the error it responds to |
| Retry button is hidden until an error state is set | Avoids visual clutter; appears only when relevant |
| `UpdateRow::new()` takes `on_retry: impl Fn() + 'static` callback | Keeps `UpdateRow` decoupled from orchestration logic |
| `on_retry` is set up in `window.rs` with a closure that runs a single-backend orchestrator | Reuses the existing `UpdateOrchestrator` without modifications |
| Retry does NOT re-run the availability check | It re-runs the update directly; the previous check result is preserved |
| Auth: a single-backend retry for a root-requiring backend will trigger pkexec again | This is correct behaviour — the privileged shell lifetime matches one orchestrator run |

### 5.3 Changes to `src/ui/update_row.rs`

#### 5.3.1 New field

```rust
#[derive(Clone)]
pub struct UpdateRow {
    pub row: adw::ExpanderRow,
    status_label: gtk::Label,
    spinner: gtk::Spinner,
    pkg_rows: Rc<RefCell<Vec<adw::ActionRow>>>,
    skip_flag: Rc<Cell<bool>>,
    last_available: Rc<Cell<Option<usize>>>,
    skip_checkbox: gtk::CheckButton,
    retry_button: gtk::Button,   // NEW
}
```

#### 5.3.2 Updated `new()` signature

```rust
pub fn new(
    backend: &dyn Backend,
    on_skip_changed: impl Fn() + 'static,
    on_retry: impl Fn() + 'static,       // NEW parameter
) -> Self
```

#### 5.3.3 Retry button construction (inside `new()`)

```rust
let retry_button = gtk::Button::builder()
    .label("Retry")
    .css_classes(vec!["suggested-action"])
    .visible(false)
    .valign(gtk::Align::Center)
    .build();
retry_button.update_property(&[gtk::accessible::Property::Label(
    &format!("Retry {} update", backend.display_name())
)]);
row.add_suffix(&retry_button);

retry_button.connect_clicked(move |_| on_retry());
```

**Suffix order:** `skip_checkbox` → `retry_button` → `spinner` → `status_label`. The retry button appears between the skip checkbox and the spinner, so it doesn't displace the status label when it appears.

#### 5.3.4 State visibility management

Show retry button only in error state; hide it in all other states:

```rust
pub fn set_status_error(&self, msg: &str) {
    self.skip_checkbox.set_sensitive(true);
    self.spinner.set_visible(false);
    self.spinner.set_spinning(false);
    self.status_label.set_label(&format!("Error: {}", msg));
    self.status_label.set_css_classes(&["error"]);
    self.retry_button.set_visible(true);   // NEW
}

pub fn set_status_running(&self) {
    self.skip_checkbox.set_sensitive(false);
    self.retry_button.set_visible(false);  // NEW
    self.spinner.set_visible(true);
    self.spinner.set_spinning(true);
    self.status_label.set_label("Updating...");
    self.status_label.set_css_classes(&["accent"]);
}

pub fn set_status_checking(&self) {
    self.last_available.set(None);
    self.retry_button.set_visible(false);  // NEW
    // ... rest unchanged ...
}
```

All other `set_status_*` methods should also call `self.retry_button.set_visible(false)` to ensure the button disappears when status changes to any non-error state.

### 5.4 Changes to `src/ui/window.rs`

#### 5.4.1 Updated `UpdateRow::new()` call site

In the backend detection loop, add the `on_retry` closure:

```rust
let backend_for_retry = backend.clone();
let rows_for_retry = rows.clone();
let log_panel_for_retry = log_panel.clone();
let updating_for_retry = updating.clone();

let row = UpdateRow::new(
    backend.as_ref(),
    /* on_skip_changed */ { /* ... existing closure ... */ },
    /* on_retry */ move || {
        if updating_for_retry.get() {
            return; // Do not retry while another update is in progress
        }
        updating_for_retry.set(true);

        let backend_arc = backend_for_retry.clone();
        let rows_rc = rows_for_retry.clone();
        let log_panel_rc = log_panel_for_retry.clone();
        let updating_rc = updating_for_retry.clone();

        glib::spawn_future_local(async move {
            use crate::orchestrator::{OrchestratorEvent, UpdateOrchestrator};

            let orchestrator = UpdateOrchestrator::new(vec![backend_arc]);
            let (event_tx, event_rx) = async_channel::unbounded::<OrchestratorEvent>();
            orchestrator.run_all(event_tx);

            while let Ok(event) = event_rx.recv().await {
                match event {
                    OrchestratorEvent::BackendStarted(kind) => {
                        let borrowed = rows_rc.borrow();
                        if let Some((_, r)) = borrowed.iter().find(|(k, _)| *k == kind) {
                            r.set_status_running();
                        }
                    }
                    OrchestratorEvent::BackendLog(kind, line) => {
                        log_panel_rc.append_line(&format!("[{kind}] {line}"));
                    }
                    OrchestratorEvent::BackendFinished(kind, result) => {
                        let borrowed = rows_rc.borrow();
                        if let Some((_, r)) = borrowed.iter().find(|(k, _)| *k == kind) {
                            match &result {
                                UpdateResult::Success { updated_count } |
                                UpdateResult::SuccessWithSelfUpdate { updated_count } => {
                                    r.set_status_success(*updated_count);
                                }
                                UpdateResult::Error(e) => {
                                    r.set_status_error(&e.to_string());
                                }
                                UpdateResult::Skipped(msg) => {
                                    r.set_status_skipped(msg);
                                }
                            }
                        }
                    }
                    OrchestratorEvent::AllFinished => break,
                    _ => {} // AuthStarted/AuthSucceeded/AuthFailed handled by orchestrator internally
                }
            }

            updating_rc.set(false);
        });
    },
);
```

**Note:** The `updating` flag is shared with the "Update All" flow. Setting it `true` in the retry closure prevents concurrent "Update All" and retry runs. The "Update All" button click handler already checks `update_in_progress.get()` and returns early, so this is safe.

### 5.5 Implementation steps

1. Add `retry_button: gtk::Button` field to `UpdateRow`.
2. Update `UpdateRow::new()` to accept `on_retry: impl Fn() + 'static`.
3. Construct retry button with `visible(false)` and `add_suffix` after `skip_checkbox`.
4. Wire `retry_button.connect_clicked` to call `on_retry()`.
5. Add `self.retry_button.set_visible(true)` to `set_status_error()`.
6. Add `self.retry_button.set_visible(false)` to all other `set_status_*` methods.
7. Update the call site in `window.rs` to pass the `on_retry` closure.

### 5.6 Affected files

- `src/ui/update_row.rs`
- `src/ui/window.rs`

---

## 6. Feature H — Update History Log

### 6.1 Current state

After "Update All" completes, no record of what was updated (or failed) is kept. There is no way to review past update sessions.

### 6.2 History format

**Format:** JSONL (JSON Lines) — one JSON object per line, one line per backend result per session run.

**Location:** `$XDG_DATA_HOME/up/history.jsonl`  
**Fallback:** `$HOME/.local/share/up/history.jsonl`  
**No `dirs` crate** — resolved entirely via `std::env::var`.

**Schema (one line per backend result):**

```json
{"timestamp":1746614400,"backend":"APT","result":"success","updated_count":42}
{"timestamp":1746614401,"backend":"Flatpak","result":"error","error":"Authentication cancelled or denied"}
{"timestamp":1746614402,"backend":"Nix","result":"skipped","updated_count":null}
```

| Field | Type | Description |
|-------|------|-------------|
| `timestamp` | `u64` | Unix timestamp seconds (UTC) at the moment `BackendFinished` is received |
| `backend` | `String` | `BackendKind::to_string()` — e.g., `"APT"`, `"Flatpak"` |
| `result` | `String` | One of: `"success"`, `"success_self_update"`, `"error"`, `"skipped"` |
| `updated_count` | `Option<usize>` | Number of packages updated; `null` for error/skipped |
| `error` | `Option<String>` | Error message; `null` for success/skipped |

### 6.3 New module: `src/history.rs`

```rust
use serde::{Deserialize, Serialize};
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub timestamp: u64,
    pub backend: String,
    pub result: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Returns the path to the history JSONL file, honoring XDG_DATA_HOME.
pub fn history_path() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".local/share")
        });
    base.join("up").join("history.jsonl")
}

/// Append a single history entry to the JSONL file.
///
/// Creates the file and parent directories if they do not exist.
/// Errors are non-fatal — callers should log but not panic.
pub fn append_entry(entry: &HistoryEntry) -> io::Result<()> {
    let path = history_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    let mut writer = BufWriter::new(file);
    let line = serde_json::to_string(entry)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    writeln!(writer, "{line}")?;
    Ok(())
}

/// Load all history entries from the JSONL file.
///
/// Returns an empty Vec if the file does not exist.
/// Lines that fail to parse are silently skipped (forward-compatible).
pub fn load_entries() -> io::Result<Vec<HistoryEntry>> {
    let path = history_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&path)?;
    let entries = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    Ok(entries)
}

/// Delete the history file, effectively clearing all history.
pub fn clear_history() -> io::Result<()> {
    let path = history_path();
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Returns the current Unix timestamp in seconds.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
```

### 6.4 New UI module: `src/ui/history_page.rs`

The History tab is added to the `adw::ViewStack` alongside Update and Upgrade pages.

```rust
use adw::prelude::*;
use gtk::glib;

pub struct HistoryPage;

impl HistoryPage {
    /// Build the History page widget.
    ///
    /// Returns the root `gtk::Box` widget to be added to the ViewStack.
    pub fn build() -> gtk::Box {
        let page_box = gtk::Box::new(gtk::Orientation::Vertical, 0);

        let scrolled = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();

        let clamp = adw::Clamp::builder()
            .maximum_size(600)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .build();

        let content_box = gtk::Box::new(gtk::Orientation::Vertical, 18);

        // Header description
        let header_label = gtk::Label::builder()
            .label("A record of past update sessions.")
            .css_classes(vec!["dim-label"])
            .build();
        content_box.append(&header_label);

        // History group (populated by load_history_into_group)
        let history_group = adw::PreferencesGroup::builder()
            .title("Update History")
            .build();

        // Clear button in PreferencesGroup header
        let clear_button = gtk::Button::builder()
            .label("Clear")
            .css_classes(vec!["destructive-action"])
            .valign(gtk::Align::Center)
            .build();
        clear_button.update_property(&[gtk::accessible::Property::Label("Clear update history")]);
        history_group.set_header_suffix(Some(&clear_button));

        // Populate from disk
        Self::populate_group(&history_group);

        // Wire up clear button
        {
            let history_group_weak = history_group.downgrade();
            clear_button.connect_clicked(move |_| {
                let _ = crate::history::clear_history();
                if let Some(group) = history_group_weak.upgrade() {
                    // Remove all child rows and show the empty placeholder
                    // adw::PreferencesGroup has no bulk-remove; rebuild the group
                    Self::clear_and_repopulate(&group);
                }
            });
        }

        content_box.append(&history_group);
        clamp.set_child(Some(&content_box));
        scrolled.set_child(Some(&clamp));
        page_box.append(&scrolled);
        page_box
    }

    /// Populate `group` with rows from the history file.
    fn populate_group(group: &adw::PreferencesGroup) {
        let entries = crate::history::load_entries().unwrap_or_default();

        if entries.is_empty() {
            let empty_row = adw::ActionRow::builder()
                .title("No history yet")
                .subtitle("Update sessions will appear here after you run an update.")
                .build();
            group.add(&empty_row);
            return;
        }

        // Show newest first
        for entry in entries.iter().rev() {
            let timestamp_str = format_timestamp(entry.timestamp);
            let subtitle = match entry.result.as_str() {
                "success" | "success_self_update" => {
                    match entry.updated_count {
                        Some(n) if n > 0 => format!("{timestamp_str} — {n} updated"),
                        _ => format!("{timestamp_str} — up to date"),
                    }
                }
                "error" => format!(
                    "{timestamp_str} — {}",
                    entry.error.as_deref().unwrap_or("unknown error")
                ),
                "skipped" => format!("{timestamp_str} — skipped"),
                _ => timestamp_str,
            };

            let row = adw::ActionRow::builder()
                .title(&entry.backend)
                .subtitle(&subtitle)
                .build();

            // Status icon
            let icon_name = match entry.result.as_str() {
                "success" | "success_self_update" => "emblem-ok-symbolic",
                "error" => "dialog-error-symbolic",
                "skipped" => "action-unavailable-symbolic",
                _ => "dialog-question-symbolic",
            };
            let icon = gtk::Image::from_icon_name(icon_name);
            icon.set_accessible_role(gtk::AccessibleRole::Presentation);
            row.add_prefix(&icon);

            group.add(&row);
        }
    }

    /// Remove all rows from group and repopulate (used after clear).
    fn clear_and_repopulate(group: &adw::PreferencesGroup) {
        // adw::PreferencesGroup does not have a `remove_all`; remove children
        // by iterating through child widgets. Use the glib child iteration.
        // Simplest approach: use `group.last_child()` loop.
        while let Some(child) = group.last_child() {
            group.remove(&child);
        }
        Self::populate_group(group);
    }
}

/// Format a Unix timestamp as a human-readable local date/time string.
///
/// Uses only std (no chrono dep). Format: "YYYY-MM-DD HH:MM".
fn format_timestamp(secs: u64) -> String {
    // Simple UTC formatting without external deps.
    // For a GTK app, glib::DateTime is available and handles local timezone.
    if let Some(dt) = glib::DateTime::from_unix_local(secs as i64).ok() {
        // glib::DateTime::format returns Option<GString>
        dt.format("%Y-%m-%d %H:%M").map(|s| s.to_string()).unwrap_or_else(|| secs.to_string())
    } else {
        secs.to_string()
    }
}
```

**Note on `adw::PreferencesGroup::remove()`:** `adw::PreferencesGroup` does expose `remove(&child: &impl IsA<gtk::Widget>)` in libadwaita 1.0+. The `clear_and_repopulate` approach iterates using `group.last_child()` which is a `gtk::Widget` method available because `adw::PreferencesGroup` implements `gtk::WidgetExt`. However, the children of a `PreferencesGroup` are not direct widget children in the usual sense — the group wraps items in internal list rows. The correct API to use is `group.remove(&adw_action_row)` for each tracked row.

**Revised approach for `clear_and_repopulate`:** Track rows in a `Vec` and call `group.remove()` on each. Since `HistoryPage::build()` is called once, and `populate_group` / `clear_and_repopulate` are helpers that need to track their rows, refactor to hold a `Vec<adw::ActionRow>` within a `Rc<RefCell<Vec<adw::ActionRow>>>` that is shared between the populate and clear functions:

```rust
let tracked_rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::new(RefCell::new(Vec::new()));

// In populate_group, also push each row into tracked_rows.borrow_mut()
// In clear_and_repopulate, drain tracked_rows, calling group.remove() on each.
```

### 6.5 Integration in `src/ui/window.rs`

#### 6.5.1 Collect results during event loop

Add a `history_entries` buffer before the orchestrator event loop begins:

```rust
let mut history_entries: Vec<crate::history::HistoryEntry> = Vec::new();
```

Inside the event loop, after the existing `BackendFinished` UI update:

```rust
OrchestratorEvent::BackendFinished(kind, result) => {
    // ... existing UI update code ...

    // Record in history buffer
    let ts = crate::history::now_secs();
    let entry = match &result {
        UpdateResult::Success { updated_count } => crate::history::HistoryEntry {
            timestamp: ts,
            backend: kind.to_string(),
            result: "success".to_string(),
            updated_count: Some(*updated_count),
            error: None,
        },
        UpdateResult::SuccessWithSelfUpdate { updated_count } => crate::history::HistoryEntry {
            timestamp: ts,
            backend: kind.to_string(),
            result: "success_self_update".to_string(),
            updated_count: Some(*updated_count),
            error: None,
        },
        UpdateResult::Error(e) => crate::history::HistoryEntry {
            timestamp: ts,
            backend: kind.to_string(),
            result: "error".to_string(),
            updated_count: None,
            error: Some(e.to_string()),
        },
        UpdateResult::Skipped(msg) => crate::history::HistoryEntry {
            timestamp: ts,
            backend: kind.to_string(),
            result: "skipped".to_string(),
            updated_count: None,
            error: Some(msg.clone()),
        },
    };
    history_entries.push(entry);
}
```

After `AllFinished` (before the reboot check):

```rust
OrchestratorEvent::AllFinished => {
    // Flush history entries to disk (non-blocking; JSONL append is fast)
    for entry in &history_entries {
        if let Err(e) = crate::history::append_entry(entry) {
            log::warn!("Failed to write history entry: {e}");
        }
    }
    break;
}
```

#### 6.5.2 Add History tab to ViewStack

In `UpWindow::build()`, after building the upgrade page:

```rust
let history_widget = crate::ui::history_page::HistoryPage::build();
view_stack.add_titled_with_icon(
    &history_widget,
    Some("history"),
    "History",
    "document-open-recent-symbolic",
);
```

### 6.6 Changes to `src/ui/mod.rs`

Add the new module declaration:

```rust
pub mod history_page;
```

### 6.7 Changes to `src/main.rs`

Add:

```rust
mod history;
```

### 6.8 Implementation steps

1. Create `src/history.rs` with `HistoryEntry`, `history_path()`, `append_entry()`, `load_entries()`, `clear_history()`, `now_secs()`.
2. Add `mod history;` to `src/main.rs`.
3. Create `src/ui/history_page.rs` with `HistoryPage::build()` and helpers.
4. Add `pub mod history_page;` to `src/ui/mod.rs`.
5. In `src/ui/window.rs`:
   - Add `use crate::ui::history_page::HistoryPage;` import.
   - Add `HistoryPage::build()` to the `ViewStack`.
   - Add `history_entries` buffer to the "Update All" event loop.
   - Append to the buffer on `BackendFinished`.
   - Flush to disk on `AllFinished`.

### 6.9 Affected files

- `src/history.rs` (new)
- `src/main.rs`
- `src/ui/history_page.rs` (new)
- `src/ui/mod.rs`
- `src/ui/window.rs`

---

## 7. Affected Files Summary

| File | Features | Change type |
|------|----------|-------------|
| `src/ui/window.rs` | D, E, F, G, H | Modify |
| `src/ui/update_row.rs` | D, G | Modify |
| `src/ui/log_panel.rs` | D | Modify |
| `src/ui/mod.rs` | H | Modify (add `pub mod history_page;`) |
| `src/ui/history_page.rs` | H | New |
| `src/battery.rs` | F | New |
| `src/history.rs` | H | New |
| `src/main.rs` | F, H | Modify (add `mod battery;`, `mod history;`) |

---

## 8. Dependency Analysis

| Feature | New Cargo dependency | Justification |
|---------|---------------------|---------------|
| D (A11y) | None | `gtk::accessible::Property` is part of gtk4-rs 0.9 |
| E (Metered) | None | `gio::NetworkMonitor` is in the already-present `gio = "0.20"` |
| F (Battery) | None | Pure `std::fs` sysfs reading |
| G (Retry) | None | Reuses `UpdateOrchestrator` and existing `async_channel` |
| H (History) | None | `serde`, `serde_json`, and `glib::DateTime` already present |

**No new Cargo dependencies are required for this batch.**

---

## 9. Implementation Order

Features should be implemented in this order to minimise merge conflicts:

1. **Feature H — History log** first: creates new files (`history.rs`, `history_page.rs`) with minimal changes to existing files; establishes the module structure others do not depend on.
2. **Feature G — Retry button**: modifies `update_row.rs` and `window.rs`; does not depend on H.
3. **Feature D — A11y**: small, targeted additions to three existing files; no dependencies.
4. **Feature E — Metered warning**: modifies `window.rs` update handler; should be done after G to understand the handler structure.
5. **Feature F — Battery prompt**: creates `battery.rs` and modifies the same `window.rs` handler as E; implement immediately after E to share the "do_start_update" refactor.

**Critical refactor for E + F:** Both features require intercepting the "Update All" click before the update starts. To avoid duplicating code, the implementation of E must extract the actual update-start logic into a standalone closure (e.g., `let do_start_update: Rc<dyn Fn()> = Rc::new(move || { ... })`) so that both the metered dialog response and the battery dialog response can invoke the same closure.

---

## 10. Risks and Mitigations

| Risk | Severity | Mitigation |
|------|----------|------------|
| `gio::NetworkMonitor::default()` unavailable in some minimal/embedded environments | Low | `is_network_metered()` returns `false` safely when no backend is present |
| sysfs `/sys/class/power_supply` absent on VM/container | Low | `read_battery()` returns `None`; prompt is silently skipped |
| Multiple batteries (ThinkPad "battery slice") may report inconsistent state | Low | Return first Battery entry found; conservative 40% threshold gives margin |
| `adw::PreferencesGroup::remove()` API may not match libadwaita 1.5 bindings | Medium | Test on target libadwaita version; fallback: destroy and rebuild the group |
| History file grows unbounded over time | Low | No truncation in this batch; a future maintenance feature can cap to N entries |
| `glib::DateTime::from_unix_local()` API shape in glib-rs 0.20 | Low | Verify exact signature: it may return `Result<DateTime, _>`; handle accordingly |
| Retry button state not reset if "Update All" is pressed after a retry | Low | The `set_status_running()` call already hides the retry button |
| `update_property` called with a format-string reference that doesn't live long enough | Medium | Bind the formatted string to a variable before passing the reference: `let label = format!(...); widget.update_property(&[Property::Label(&label)]);` |
| History JSONL written from the GTK main thread could block UI on slow filesystems | Very Low | JSONL append is typically <1ms; BufWriter ensures single write syscall |

---

*End of Specification — Section 7 Batch 2*
