# Specification: Backlog Item #12 — Per-backend Skip Checkboxes + Preview / Dry-run Button

**Feature name:** `skip_checkboxes_preview`  
**Spec file:** `.github/docs/subagent_docs/skip_checkboxes_preview_spec.md`  
**Date:** 2026-05-08  
**Status:** READY FOR IMPLEMENTATION

---

## Executive Summary

After a thorough codebase audit, **the vast majority of Backlog Item #12 is already fully implemented**. Both the per-backend skip checkboxes and the preview/package-list infrastructure exist and are wired end-to-end. The single missing piece is **persistence of skip checkbox state across app restarts**. This spec focuses exclusively on that gap.

---

## 1. Current State Analysis

### 1.1 Skipped State — Where and What

`Skipped` appears in two places in the codebase:

**`src/backends/mod.rs` — `UpdateResult` enum:**
```rust
pub enum UpdateResult {
    Success { updated_count: usize },
    SuccessWithSelfUpdate { updated_count: usize },
    Error(BackendError),
    #[allow(dead_code)]
    Skipped(String),
}
```
This variant is the backend result type. It is `#[allow(dead_code)]` because no backend's `run_update()` currently emits it; skipping is handled at the UI layer before the orchestrator is invoked.

**`src/ui/update_row.rs` — `UpdateRow` struct:**
```rust
pub struct UpdateRow {
    pub row: adw::ExpanderRow,
    status_label: gtk::Label,
    spinner: gtk::Spinner,
    pkg_rows: Rc<RefCell<Vec<adw::ActionRow>>>,
    skip_flag: Rc<Cell<bool>>,              // ← runtime in-memory skip flag
    last_available: Rc<Cell<Option<usize>>>,
    skip_checkbox: gtk::CheckButton,        // ← UI toggle (already present)
    retry_button: gtk::Button,
}
```

The `skip_checkbox` is already a fully wired `gtk::CheckButton` added as a row suffix with:
- Tooltip: `"Skip {backend.display_name()} during Update All"`
- Accessible label set via `update_property(&[gtk::accessible::Property::Label(...)])`
- `connect_toggled` handler that sets `skip_flag`, updates `status_label` to "Skipped" / restores previous count, and calls `on_skip_changed()`

**Visual state of Skipped:**  
`set_status_skipped(msg: &str)` → hides retry button, hides spinner, sets `status_label` to `msg` with `dim-label` CSS class, leaves `skip_checkbox` sensitive.

### 1.2 Orchestrator Skip Filtering

In `src/ui/window.rs`, the Update All button handler already:
1. Iterates all rows and calls `row.set_status_skipped("Skipped by user")` for any row where `row.is_skipped()` is true.
2. Builds the `backends: Vec<Arc<dyn Backend>>` by filtering out backends whose corresponding row returns `is_skipped() == true`.
3. Passes only the non-skipped backends to `UpdateOrchestrator::new(backends).run_all(event_tx)`.

The same skip filtering is applied for Maintenance (via `CleanupOrchestrator`).

### 1.3 `list_available` / Preview Infrastructure

`Backend::list_available()` exists on all backends and returns `Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>>`:

| Backend        | Command                                          | Returns               |
|----------------|--------------------------------------------------|-----------------------|
| `AptBackend`   | `apt list --upgradable`                          | package names         |
| `DnfBackend`   | `dnf check-update`                               | package names         |
| `PacmanBackend`| `pacman -Qu`                                     | package names         |
| `ZypperBackend`| `zypper list-updates`                            | package names         |
| `FlatpakBackend`| `flatpak remote-ls --updates --user --columns=application` | app IDs |
| `HomebrewBackend`| `brew outdated`                                | formula names         |
| `NixBackend`   | flake changed inputs / determinate version check / `nix-env -u --dry-run` | names or empty |

Default trait impl returns `Ok(Vec::new())` for forward compatibility.

`count_available()` is a separate trait method that delegates to `list_available().map(|v| v.len())` by default.

**Preview is already running:** The Refresh button (header) and app startup both trigger `run_checks`, which calls `count_available()` + `list_available()` on every backend in parallel. Results are displayed via:
- `row.set_status_available(count)` → updates the status label
- `row.set_packages(&packages)` → fills the `adw::ExpanderRow` child rows (capped at 50 with "… and N more")

### 1.4 UpdateRow Widget Layout

Each `UpdateRow.row` is an `adw::ExpanderRow` placed inside an `adw::PreferencesGroup` named "Sources".

Suffix layout (right-to-left in the row):
1. `status_label` — "Ready" / "N available" / "Updating…" / "Up to date" / "Skipped" / "Error: …"
2. `spinner` — `gtk::Spinner`, shown during checking/updating
3. `retry_button` — icon button, shown on error
4. `skip_checkbox` — `gtk::CheckButton`, always visible

Child rows (inside the expander):
- Up to 50 `adw::ActionRow` items showing package names
- An optional "… and N more" summary row

### 1.5 State Persistence — Current State

**There is no persistence.** Every app restart resets all `skip_flag` values to `false`. No GSettings schema exists in `data/`. No config file infrastructure exists in `src/`.

The only storage infrastructure present is `src/history.rs`, which writes JSONL to `$XDG_DATA_HOME/up/history.jsonl`.

---

## 2. Feature Definition

### 2A. Per-backend Skip Checkboxes (ALREADY IMPLEMENTED — needs persistence only)

**Already done:**
- Toggle UI (`gtk::CheckButton`) in each `UpdateRow` suffix
- Visual "Skipped" state in the row
- Orchestrator skip filtering (backends excluded from Update All and Maintenance)
- On-skip callback that recalculates the "Update All" button sensitivity

**Missing:**
- Load saved skip state on app startup and apply to newly created `UpdateRow` widgets
- Save skip state whenever any checkbox is toggled

### 2B. Preview / Dry-run Button (ALREADY IMPLEMENTED — no changes needed)

The existing Refresh button (header, icon `view-refresh-symbolic`) already:
- Calls `list_available()` on all detected backends (unprivileged)
- Populates package name lists in the `adw::ExpanderRow` children
- Shows "Up to date" when count is 0
- Shows "N available" in accent color when updates exist
- Does NOT call `run_update()`

No separate "Preview" button is needed. The Refresh button IS the preview mechanism.

---

## 3. Architecture & Data Flow

### 3.1 Persistence Layer Design

**Strategy: JSON config file at `$XDG_CONFIG_HOME/up/config.json`**

Rationale for JSON over GSettings:
- `serde` and `serde_json` are already in `Cargo.toml` — zero new dependencies
- Follows the established pattern of `src/history.rs` (`XDG_DATA_HOME`)
- No Meson build system changes required
- No GSettings XML schema file to maintain
- No `glib-compile-schemas` invocation needed
- GSettings is more appropriate for user-facing toggleable preferences accessible from multiple tools; this is an internal app state preference

**Config structure:**
```rust
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AppConfig {
    /// BackendKind variants of backends the user has checked "skip".
    #[serde(default)]
    pub skipped_backends: Vec<BackendKind>,
}
```

`BackendKind` already derives `Serialize` and `Deserialize` from serde (confirmed in `src/backends/mod.rs`).

**Config file path:**
```
$XDG_CONFIG_HOME/up/config.json   (default: ~/.config/up/config.json)
```

### 3.2 Data Flow on Startup

```
app launch
  → src/main.rs: load AppConfig from config.json (or default)
  → pass skipped_backends list down to UpWindow::build()
  → for each newly created UpdateRow:
      if backend.kind() ∈ skipped_backends:
          skip_checkbox.set_active(true)   ← triggers connect_toggled
```

### 3.3 Data Flow on Toggle

```
user clicks skip_checkbox on backend X
  → connect_toggled fires
  → skip_flag.set(true/false)
  → status_label updated
  → on_skip_changed() callback called
  → window collects current skipped list from all rows
  → save_config(AppConfig { skipped_backends }) to config.json
```

The save is synchronous on the GTK main thread. The config file is small (< 1 KB) so blocking is acceptable. If needed, it can be dispatched off-thread via `spawn_background_async`, but it is not required.

### 3.4 No Orchestrator Changes Required

The orchestrator (`src/orchestrator.rs`) does not need to change. Skip filtering already happens in `window.rs` before backends are passed to the orchestrator.

---

## 4. Implementation Steps (Ordered)

### Step 1 — Create `src/config.rs`

New file. Provides load/save for `AppConfig`.

```rust
use crate::backends::BackendKind;
use serde::{Deserialize, Serialize};
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub skipped_backends: Vec<BackendKind>,
}

/// Returns the path to the config JSON file, honoring XDG_CONFIG_HOME.
pub fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".config")
        });
    base.join("up").join("config.json")
}

/// Load the application config. Returns `AppConfig::default()` on any error
/// (missing file, parse error) to ensure a clean startup every time.
pub fn load_config() -> AppConfig {
    let path = config_path();
    if !path.exists() {
        return AppConfig::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => AppConfig::default(),
    }
}

/// Persist the application config to disk.
/// Creates parent directories if they don't exist.
/// Errors are non-fatal; callers should log but not panic.
pub fn save_config(config: &AppConfig) -> io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)?;
    let mut writer = BufWriter::new(file);
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write!(writer, "{json}")?;
    Ok(())
}
```

**File:** `src/config.rs` (new)

---

### Step 2 — Register module in `src/main.rs`

Add `mod config;` to the module list in `src/main.rs`.

**File:** `src/main.rs`  
**Change:** Add `mod config;` after existing `mod` declarations.

---

### Step 3 — Pass initial skip list to `UpWindow::build()`

Modify `src/app.rs` to load the config and pass `skipped_backends` to `UpWindow::build()`.

**File:** `src/app.rs`

Change `UpWindow::build(app)` call in `on_activate` to:
```rust
fn on_activate(app: &adw::Application) {
    // ... existing icon theme setup ...
    let config = crate::config::load_config();
    let window = UpWindow::build(app, config.skipped_backends);
    window.present();
}
```

---

### Step 4 — Update `UpWindow::build()` signature

**File:** `src/ui/window.rs`

Change signature:
```rust
pub fn build(app: &adw::Application, initial_skipped: Vec<BackendKind>) -> adw::ApplicationWindow {
```

Pass `initial_skipped` into the detection callback closure (Step 5) and into the `on_skip_changed` closure (Step 6).

---

### Step 5 — Apply initial skip state when rows are created

In `window.rs`, inside the `glib::spawn_future_local` closure that runs after `detect_rx.recv()` (where `UpdateRow::new(...)` is called for each backend), apply the initial skip state immediately after creating each row:

```rust
let row = UpdateRow::new(
    backend.as_ref(),
    on_skip_changed_closure,
    on_retry_closure,
);

// Apply persisted skip state before adding to the group.
if initial_skipped.contains(&backend.kind()) {
    row.skip_checkbox.set_active(true);
    // connect_toggled fires automatically; skip_flag is set by the handler.
}

backends_group.add(&row.row);
rows_mut.push((backend.kind(), row));
```

> **Note:** `initial_skipped` must be cloned into the async closure. Since `BackendKind` is `Copy`, `initial_skipped.clone()` (which is already `Vec<BackendKind>`) works cleanly.

> **Note:** `UpdateRow::skip_checkbox` is currently a private field. It must be made `pub(crate)` or a new method `pub fn set_skipped(&self, skipped: bool)` must be added to `UpdateRow`. The method approach is preferred for encapsulation:

```rust
/// Set the skip state programmatically (e.g., to restore persisted state).
/// Triggers the same visual update and on_skip_changed callback as a user click.
pub fn set_skipped(&self, skipped: bool) {
    self.skip_checkbox.set_active(skipped);
}
```

Add this method to `src/ui/update_row.rs`.

---

### Step 6 — Save config when skip state changes

The `on_skip_changed` closure in `window.rs` is called whenever any skip checkbox is toggled. Extend it to collect the current skipped backends and persist them:

```rust
let on_skip_changed = {
    let rows = rows.clone();
    let updating = updating.clone();
    let update_button = update_button.clone();
    move || {
        if updating.get() {
            return;
        }
        // Recalculate button sensitivity (existing logic).
        let borrowed = rows.borrow();
        let non_skipped_available: usize = borrowed
            .iter()
            .filter(|(_, r)| !r.is_skipped())
            .filter_map(|(_, r)| r.last_available_count())
            .sum();
        update_button.set_sensitive(non_skipped_available > 0);

        // Persist new skip state.
        let skipped: Vec<BackendKind> = borrowed
            .iter()
            .filter(|(_, r)| r.is_skipped())
            .map(|(k, _)| *k)
            .collect();
        drop(borrowed);
        let config = crate::config::AppConfig { skipped_backends: skipped };
        if let Err(e) = crate::config::save_config(&config) {
            log::warn!("Failed to save skip config: {e}");
        }
    }
};
```

> **Note:** The current `on_skip_changed` callback in the detection closure is a separate per-row closure. It currently captures only `rows_cb`, `button_cb`, and `updating_cb`. This step restructures it to also capture the config-save logic. The simplest approach is an `Rc<dyn Fn()>` shared across all rows (same as `run_checks`), or inline the save inside each per-row closure.

> **Recommended:** Make `on_skip_changed` a shared `Rc<dyn Fn()>` created once before the detection loop, similar to `run_checks`. All row closures reference the same `Rc`.

---

### Step 7 — `UpdateRow::set_skipped` visibility for `initial_skipped` (from Step 5)

`UpdateRow::skip_checkbox` must be accessible from `window.rs`. Add to `src/ui/update_row.rs`:

```rust
/// Restore persisted skip state on startup.
/// Calling this with `true` fires the same `connect_toggled` handler
/// as a user interaction, ensuring skip_flag and status_label are consistent.
pub fn set_skipped(&self, skipped: bool) {
    self.skip_checkbox.set_active(skipped);
}
```

This is the only change needed in `update_row.rs`.

---

## 5. New Dependencies

**None.** All required crates are already in `Cargo.toml`:
- `serde = { version = "1", features = ["derive"] }` ✅
- `serde_json = "1"` ✅

`BackendKind` already has `#[derive(Serialize, Deserialize)]` applied.

---

## 6. Files Modified

| File | Change Type | Description |
|------|-------------|-------------|
| `src/config.rs` | **New** | `AppConfig` struct + `config_path()`, `load_config()`, `save_config()` |
| `src/main.rs` | Edit | Add `mod config;` |
| `src/app.rs` | Edit | Load config; pass `skipped_backends` to `UpWindow::build()` |
| `src/ui/window.rs` | Edit | Accept `initial_skipped: Vec<BackendKind>`; apply on row creation; save on toggle |
| `src/ui/update_row.rs` | Edit | Add `pub fn set_skipped(&self, skipped: bool)` |

---

## 7. Risks & Mitigations

| Risk | Severity | Mitigation |
|------|----------|------------|
| Config file unreadable / corrupted | Low | `load_config()` returns `AppConfig::default()` (all enabled) on any error |
| Save failure (disk full, permissions) | Low | Non-fatal; logged via `log::warn!`; UI state not affected |
| `connect_toggled` fires during `set_active(true)` on startup before `detected` / `rows` are fully populated | Low | `set_skipped()` is called after the row is added to `rows_mut`; `on_skip_changed` callback accesses `rows` via `Rc::borrow()` which is safe at that point |
| Skip state saved before all rows are detected | None | Config is only written in `on_skip_changed`, which is only triggered by actual user interaction or the `set_skipped()` call during post-detection init — both happen after detection is complete |
| `BackendKind` serialization format change | Low | `BackendKind` uses serde's default derive; if a new variant is added, existing configs remain valid (unknown variants are ignored by `#[serde(default)]`) |
| Race between detection async task and save | None | GTK main-loop is single-threaded for UI operations; detection completes on main thread via `glib::spawn_future_local`; no concurrent access |

---

## 8. Out of Scope (Already Implemented)

These items are confirmed complete and require no implementation work:

- ✅ Skip checkbox widget in `UpdateRow` (`gtk::CheckButton` with accessible label, suffix placement)
- ✅ Skip flag tracking (`Rc<Cell<bool>>`, `is_skipped()`, `last_available_count()`)
- ✅ Visual "Skipped" state display (`set_status_skipped()`, `dim-label` CSS)
- ✅ Un-skip restoring previous count display (inside `connect_toggled` handler)
- ✅ Orchestrator skip filtering (in `window.rs` Update All and Maintenance handlers)
- ✅ `list_available()` on all backends (APT, DNF, Pacman, Zypper, Flatpak, Homebrew, Nix)
- ✅ Package name display in `adw::ExpanderRow` child rows (via `set_packages()`, capped at 50)
- ✅ Preview/Dry-run (existing Refresh button triggers `list_available()` on all backends)
- ✅ "Up to date" / "N available" status labels
- ✅ Epoch-based stale check cancellation for concurrent refresh cycles
- ✅ Per-row retry button

---

## 9. Research Sources

1. **gtk4-rs book — Accessibility chapter** (github.com/gtk-rs/gtk4-rs) — `update_property`, `accessible::Property::Label`, `accessible::Relation::LabelledBy` patterns for accessible checkboxes
2. **gtk4-rs book — Todo app tutorial** (github.com/gtk-rs/gtk4-rs) — `adw::ActionRow` + `gtk::CheckButton` as `activatable_widget` pattern
3. **libadwaita API docs v1.5** (gnome.pages.gitlab.gnome.org/libadwaita) — Confirmed `adw::ExpanderRow::add_row`, `adw::ActionRow::builder`, `adw::ExpanderRow::set_enable_expansion`; `adw::Spinner` is v1.6+ only
4. **GNOME HIG — Toggles** (developer.gnome.org/hig) — CheckButton appropriate for inline item-level toggles; SwitchRow for page-level settings
5. **XDG Base Directory Specification** (specifications.freedesktop.org) — `$XDG_CONFIG_HOME` default `~/.config`; appropriate for per-user app configuration
6. **Existing `src/history.rs`** — Established project pattern for XDG-based storage with serde_json; `create_dir_all` + `OpenOptions::new().truncate(true)` pattern for config writes
7. **Context7 gtk4-rs library docs** — Confirmed `gtk::CheckButton::set_active()` triggers `connect_toggled`; `update_property(&[gtk::accessible::Property::Label(...)])` is the correct accessibility API for gtk4-rs v0.9

---

## 10. Summary

**The total implementation is small**: 1 new file (`src/config.rs`, ~50 lines), edits to 4 existing files. The primary work is in `window.rs` to thread the initial skip list through and wire the save callback.

No new Cargo dependencies are needed. No Meson build changes are needed. No GSettings schema is needed. The implementation follows the exact same XDG + serde_json pattern already established by `src/history.rs`.
