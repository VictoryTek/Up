# Per-Item Checkboxes — Feature Specification

**Project:** Up — GTK4/libadwaita Linux system updater (Rust, Edition 2021)  
**Feature:** Per-item package checkboxes inside each backend's expander row  
**Spec version:** 1.0  
**Date:** 2026-05-13

---

## Table of Contents

1. [Current State Analysis](#1-current-state-analysis)
2. [Problem Definition](#2-problem-definition)
3. [Research Sources](#3-research-sources)
4. [Proposed Solution Architecture](#4-proposed-solution-architecture)
5. [Per-Backend Support Matrix](#5-per-backend-support-matrix)
6. [Implementation Steps (Ordered)](#6-implementation-steps-ordered)
7. [Files Modified / Created](#7-files-modified--created)
8. [Risks and Mitigations](#8-risks-and-mitigations)

---

## 1. Current State Analysis

### 1.1 UI Layer — `src/ui/update_row.rs`

Each detected backend is represented by an `UpdateRow` struct built around an `adw::ExpanderRow`.

```
UpdateRow
├── adw::ExpanderRow  (title = backend name, subtitle = description)
│   ├── prefix: gtk::Image (icon)
│   ├── suffix: gtk::CheckButton  ← "skip this backend" (active = skip)
│   ├── suffix: gtk::Button       ← retry
│   ├── suffix: gtk::Spinner
│   ├── suffix: gtk::Label        ← status text
│   └── [child rows – adw::ActionRow per package, added by set_packages()]
```

Key fields on `UpdateRow`:
- `skip_flag: Rc<Cell<bool>>` — mirrors `skip_checkbox.is_active()`
- `pkg_rows: Rc<RefCell<Vec<adw::ActionRow>>>` — plain display rows, no checkboxes
- `packages_cache: Rc<RefCell<Vec<String>>>` — raw `list_available()` output

`set_packages(&[String])` populates `pkg_rows` with plain read-only `adw::ActionRow`s (max 50 shown, excess collapsed into a summary row). No interactivity.

`is_skipped() -> bool` returns `skip_flag.get()`.

### 1.2 Backend Trait — `src/backends/mod.rs`

```rust
pub trait Backend: Send + Sync {
    fn kind(&self) -> BackendKind;
    fn display_name(&self) -> &str;
    fn description(&self) -> &str;
    fn icon_name(&self) -> &str;

    fn run_update<'a>(&'a self, runner: &'a dyn CommandExecutor)
        -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>>;

    fn needs_root(&self) -> bool { false }

    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async move { self.list_available().await.map(|v| v.len()) })
    }

    fn list_available(&self) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn estimate_size(&self) -> Pin<Box<dyn Future<Output = Option<u64>> + Send + '_>> {
        Box::pin(async { None })
    }

    fn supports_cleanup(&self) -> bool { false }
    fn run_cleanup<'a>(...) -> ...;
}
```

`run_update()` takes no item-selection parameter — it always updates everything.

### 1.3 What `list_available()` Returns per Backend

| Backend | Return value of `list_available()` |
|---------|-------------------------------------|
| APT | Package names: `["htop", "curl", "libssl3"]` |
| DNF | Package names: `["htop", "curl"]` |
| Pacman | Package names: `["htop", "pacman"]` |
| Zypper | Package names: `["htop", "zypper"]` |
| Flatpak | App IDs: `["org.gnome.Calculator", "com.spotify.Client"]` |
| Homebrew | Formula names: `["htop", "curl"]` |
| Nix (NixOS flake) | Input names: `["nixpkgs", "nixos-unstable", "home-manager"]` |
| Nix (NixOS channel) | `[]` (cannot enumerate without updating) |
| Nix (profile, legacy) | Package names from `nix-env -u --dry-run` |
| Nix (profile, modern) | `[]` (no dry-run equivalent) |
| Determinate Nix | `["determinate-nix"]` if upgrade available, else `[]` |
| Fwupd | `["Unifying Receiver (RQR12.10_B0032)", ...]` — display string, not device GUID |
| Plugin | Depends on `list_available` command in YAML descriptor |

All returned strings are suitable as display labels. For Flatpak and Nix inputs they are also the precise IDs needed for selective update commands.

### 1.4 Orchestrator — `src/orchestrator.rs`

`UpdateOrchestrator` holds `Vec<Arc<dyn Backend>>` and calls `backend.run_update(runner)` for each. No mechanism for passing a subset of items.

### 1.5 Window — `src/ui/window.rs`

The "Update All" button click builds the backends list:

```rust
let backends: Vec<Arc<dyn Backend>> = {
    let detected_borrow = detected.borrow();
    let rows_borrow = rows.borrow();
    detected_borrow
        .iter()
        .filter(|b| {
            rows_borrow.iter()
                .find(|(k, _)| *k == b.kind())
                .map(|(_, r)| !r.is_skipped())
                .unwrap_or(true)
        })
        .cloned()
        .collect()
};
let orchestrator = UpdateOrchestrator::new(backends);
```

No selection state is extracted or forwarded.

---

## 2. Problem Definition

### 2.1 User Need

Users want fine-grained control over which packages are updated in a given run. Examples:

- Update `nixpkgs` but skip `nixos-unstable` (to avoid a breaking NixOS rebuild)
- Update `org.gnome.Calculator` but skip `com.spotify.Client` (large download)
- Update `htop` but skip `libssl3` (policy freeze on security libraries)

### 2.2 Current Limitation

The existing per-backend skip checkbox is binary: update everything in a backend or skip the backend entirely. There is no mechanism for selecting a subset of packages within a backend.

### 2.3 Non-Goals

- This feature does **not** change the outer backend-skip checkbox for backends that do not support per-item selection.
- This feature does **not** introduce dependency resolution UI.
- Package deselections are **not persisted** across sessions (transient UI state only).
- The `--check` daemon (`src/check.rs`) is **not affected** — it counts all available updates regardless of UI selection.

---

## 3. Research Sources

1. **gtk4-rs book – Todo app (todo_3.md)**: Demonstrates `adw::ActionRow` with `gtk::CheckButton` as activatable widget. Pattern: create `ActionRow`, set `activatable_widget` to a `CheckButton`, bind properties. URL: https://gtk-rs.org/gtk4-rs/stable/latest/book/todo_3.html

2. **GTK4 API – GtkCheckButton**: `inconsistent` property renders a dash/minus visual for indeterminate state (tri-state pattern). The `active` and `inconsistent` properties are independent; `inconsistent=true` does not change `active`. Signal: `toggled`. Blocking with `block_signal(&id)` / `unblock_signal(&id)` prevents reentrancy. URL: https://docs.gtk.org/gtk4/class.CheckButton.html

3. **Libadwaita API – AdwExpanderRow**: `add_row(widget)` appends a child row inside the expander. `remove(widget)` removes a previously added child. No limit on child row types; any `GtkWidget` accepted. URL: https://gnome.pages.gitlab.gnome.org/libadwaita/doc/main/class.ExpanderRow.html

4. **Flatpak CLI – selective update**: `flatpak update -y <app-id>` updates a specific app. Multiple app-IDs can be listed in a single invocation. Source: `man flatpak-update`.

5. **APT – selective upgrade**: `apt-get install --only-upgrade -y <pkg1> <pkg2>` upgrades specific packages without installing new ones. Requires root. Source: `man apt-get`.

6. **Nix flake selective input update**: `nix flake update <input-name>` updates a single named input in `flake.lock`. Multiple inputs can be listed. After updating inputs, `nixos-rebuild switch` applies them. Source: https://nixos.org/manual/nix/stable/command-ref/new-cli/nix3-flake-update.html

7. **Pacman partial upgrades**: The Arch Wiki explicitly states that partial upgrades are **unsupported** and can lead to broken systems. `pacman -S <pkg>` without `-Syu` can install a newer package against unmatched dependencies. Source: https://wiki.archlinux.org/title/System_maintenance#Partial_upgrades_are_unsupported

8. **DNF selective upgrade**: `dnf upgrade -y <pkg1> <pkg2>` upgrades specific packages. Source: `man dnf`.

9. **Zypper selective update**: `zypper update <pkg1> <pkg2>` updates specific packages. Source: `man zypper`.

10. **Homebrew selective upgrade**: `brew upgrade <formula1> <formula2>` upgrades specific formulae. Source: `man brew`.

11. **GTK4 signal blocking**: `glib::object::ObjectExt::block_signal(id)` / `unblock_signal(id)` on any GObject. In gtk4-rs, `connect_toggled` returns a `glib::signal::SignalHandlerId` that can be used for blocking. Pattern documented in GTK4 internals and gtk-rs examples.

---

## 4. Proposed Solution Architecture

### 4.1 Overview

Three layers are touched:

```
UI (UpdateRow) ──── selection state ────► Window (update button)
                                              │
                                              ▼
                                      UpdateOrchestrator
                                       (backends + selections)
                                              │
                                              ▼
                              backend.run_selected_update(items)
                              OR
                              backend.run_update()  [fallback]
```

### 4.2 Backend Trait Changes — `src/backends/mod.rs`

Add two new optional trait methods:

```rust
/// Whether this backend supports updating a user-specified subset of items
/// returned by `list_available()`.
///
/// When `false`, the per-item checkboxes in the UI are rendered read-only
/// (always checked, non-interactive) to indicate that the items are for
/// display only. The full `run_update()` is always used.
///
/// Default: `false`.
fn supports_item_selection(&self) -> bool {
    false
}

/// Run an update restricted to the provided item IDs.
///
/// `items` is a non-empty slice of IDs drawn from the `Vec<String>` that
/// `list_available()` returned for this backend. The implementation
/// MUST NOT perform a full system update when `items` is non-empty.
///
/// The default implementation ignores `items` and delegates to
/// `run_update()` for backward compatibility with backends that override
/// neither method.
///
/// Callers guarantee:
/// - `items.is_empty()` is never true when this method is called.
/// - All entries in `items` are strings from the most-recent
///   `list_available()` result.
fn run_selected_update<'a>(
    &'a self,
    items: &'a [String],
    runner: &'a dyn CommandExecutor,
) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
    let _ = items;
    self.run_update(runner)
}
```

**No breaking changes**: all existing backends compile with the default implementations.

### 4.3 Per-Backend Implementations

#### 4.3.1 FlatpakBackend (`src/backends/flatpak.rs`)

```rust
fn supports_item_selection(&self) -> bool { true }

fn run_selected_update<'a>(
    &'a self,
    items: &'a [String],
    runner: &'a dyn CommandExecutor,
) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
    Box::pin(async move {
        // Validate: app IDs must be non-empty and pass basic format check
        // (ASCII alphanumeric, dots, hyphens only — same as Flatpak app-ID rules).
        for id in items {
            if id.is_empty() || id.contains([' ', '\n', '\r', '\0', '\'', '"', ';', '&', '|', '`', '$', '\\']) {
                return UpdateResult::Error(BackendError::from_string(
                    format!("Invalid Flatpak app ID: {:?}", id)
                ));
            }
        }
        let mut sub_args = vec!["update", "-y"];
        sub_args.extend(items.iter().map(|s| s.as_str()));
        let (cmd, args) = build_flatpak_cmd(&sub_args);
        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        match runner.run(&cmd, &args_refs).await {
            Ok(output) => {
                let count = output.lines()
                    .filter(|l| l.trim().starts_with(|c: char| c.is_ascii_digit()))
                    .count();
                UpdateResult::Success { updated_count: count }
            }
            Err(e) => UpdateResult::Error(e),
        }
    })
}
```

#### 4.3.2 AptBackend (`src/backends/os_package_manager.rs`)

```rust
fn supports_item_selection(&self) -> bool { true }

fn run_selected_update<'a>(
    &'a self,
    items: &'a [String],
    runner: &'a dyn CommandExecutor,
) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
    Box::pin(async move {
        // Validate: package names must be safe shell tokens (letters, digits, +, -, ., _).
        for pkg in items {
            if pkg.is_empty() || pkg.chars().any(|c| !(c.is_ascii_alphanumeric()
                || c == '-' || c == '+' || c == '.' || c == '_' || c == ':'))
            {
                return UpdateResult::Error(BackendError::from_string(
                    format!("Invalid package name: {:?}", pkg)
                ));
            }
        }
        let pkg_list = items.join(" ");
        let cmd = format!(
            "DEBIAN_FRONTEND=noninteractive apt-get install --only-upgrade -y {}",
            pkg_list
        );
        match runner.run("pkexec", &["sh", "-c", &cmd]).await {
            Ok(output) => UpdateResult::Success {
                updated_count: count_apt_upgraded(&output),
            },
            Err(e) => UpdateResult::Error(e),
        }
    })
}
```

#### 4.3.3 DnfBackend (`src/backends/os_package_manager.rs`)

```rust
fn supports_item_selection(&self) -> bool { true }

fn run_selected_update<'a>(
    &'a self,
    items: &'a [String],
    runner: &'a dyn CommandExecutor,
) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
    Box::pin(async move {
        for pkg in items {
            if pkg.is_empty() || pkg.chars().any(|c| !(c.is_ascii_alphanumeric()
                || c == '-' || c == '.' || c == '_'))
            {
                return UpdateResult::Error(BackendError::from_string(
                    format!("Invalid package name: {:?}", pkg)
                ));
            }
        }
        let mut args = vec!["pkexec", "dnf", "upgrade", "-y"];
        args.extend(items.iter().map(|s| s.as_str()));
        match runner.run(args[0], &args[1..]).await {
            Ok(output) => UpdateResult::Success {
                updated_count: count_dnf_upgraded(&output),
            },
            Err(e) => UpdateResult::Error(e),
        }
    })
}
```

#### 4.3.4 PacmanBackend — **NO item selection support**

Pacman partial upgrades are explicitly unsupported by the Arch Linux project and can break the system. `supports_item_selection()` returns `false` (default). Items are shown read-only in the UI.

#### 4.3.5 ZypperBackend (`src/backends/os_package_manager.rs`)

```rust
fn supports_item_selection(&self) -> bool { true }

fn run_selected_update<'a>(
    &'a self,
    items: &'a [String],
    runner: &'a dyn CommandExecutor,
) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
    Box::pin(async move {
        for pkg in items {
            if pkg.is_empty() || pkg.chars().any(|c| !(c.is_ascii_alphanumeric()
                || c == '-' || c == '.' || c == '_'))
            {
                return UpdateResult::Error(BackendError::from_string(
                    format!("Invalid package name: {:?}", pkg)
                ));
            }
        }
        let mut args = vec!["pkexec", "zypper", "--non-interactive", "update"];
        args.extend(items.iter().map(|s| s.as_str()));
        match runner.run(args[0], &args[1..]).await {
            Ok(output) => UpdateResult::Success {
                updated_count: count_zypper_upgraded(&output),
            },
            Err(e) => UpdateResult::Error(e),
        }
    })
}
```

#### 4.3.6 HomebrewBackend (`src/backends/homebrew.rs`)

```rust
fn supports_item_selection(&self) -> bool { true }

fn run_selected_update<'a>(
    &'a self,
    items: &'a [String],
    runner: &'a dyn CommandExecutor,
) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
    Box::pin(async move {
        for formula in items {
            if formula.is_empty() || formula.chars().any(|c| !(c.is_ascii_alphanumeric()
                || c == '-' || c == '_' || c == '.' || c == '/'))
            {
                return UpdateResult::Error(BackendError::from_string(
                    format!("Invalid formula name: {:?}", formula)
                ));
            }
        }
        // Refresh metadata before upgrading selected formulae.
        if let Err(e) = runner.run("brew", &["update"]).await {
            return UpdateResult::Error(e);
        }
        let mut args = vec!["upgrade"];
        args.extend(items.iter().map(|s| s.as_str()));
        match runner.run("brew", &args).await {
            Ok(output) => UpdateResult::Success {
                updated_count: count_homebrew_upgraded(&output),
            },
            Err(e) => UpdateResult::Error(e),
        }
    })
}
```

#### 4.3.7 NixBackend — Flake inputs only (`src/backends/nix.rs`)

Selective update is only supported for **flake-based NixOS** (where `list_available()` returns flake input names). For NixOS channel, Nix profile (modern), and Determinate Nix, item selection is not supported.

```rust
fn supports_item_selection(&self) -> bool {
    is_nixos() && is_nixos_flake()
}

fn run_selected_update<'a>(
    &'a self,
    items: &'a [String],
    runner: &'a dyn CommandExecutor,
) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
    Box::pin(async move {
        // Validate all input names against the same rules as flake attributes.
        for input in items {
            if let Err(e) = validate_flake_attr(input) {
                return UpdateResult::Error(BackendError::from_string(e));
            }
        }
        let config_name = match resolve_nixos_flake_attr() {
            Ok(n) => n,
            Err(e) => return UpdateResult::Error(BackendError::from_string(e)),
        };
        // Build: nix flake update <input1> <input2> ...
        // Then: nixos-rebuild switch
        let inputs_str = items.join(" ");
        let cmd = format!(
            "stdbuf -oL -eL \
             nix --extra-experimental-features 'nix-command flakes' \
             flake update {} --flake /etc/nixos && \
             stdbuf -oL -eL \
             nixos-rebuild switch --flake /etc/nixos#{} --refresh --print-build-logs",
            inputs_str,
            config_name
        );
        match runner.run(
            "pkexec",
            &[
                "env",
                "PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin",
                "sh", "-c", &cmd,
            ],
        ).await {
            Ok(output) => UpdateResult::Success {
                updated_count: count_nix_store_operations(&output),
            },
            Err(e) => UpdateResult::Error(e),
        }
    })
}
```

**Note:** Even when only one input is updated, `nixos-rebuild switch` still runs a full system rebuild. This is unavoidable with NixOS.

#### 4.3.8 FwupdBackend — **NO item selection support**

`list_available()` returns `"DeviceName (Version)"` display strings, not the internal device GUIDs required by `fwupdmgr update <device-id>`. Adding device GUID tracking would require significant changes to `parse_fwupd_updates`. Deferred to a future enhancement.

#### 4.3.9 PluginBackend — **NO item selection support** (default)

The plugin YAML schema v1 does not define a `select_update` command. If added in a future schema version, `supports_item_selection()` can be overridden. Deferred.

---

### 4.4 Orchestrator Changes — `src/orchestrator.rs`

#### 4.4.1 Type change

`UpdateOrchestrator` changes its internal storage from `Vec<Arc<dyn Backend>>` to `Vec<(Arc<dyn Backend>, Option<Vec<String>>)>`:

```rust
pub struct UpdateOrchestrator {
    /// Each element is (backend, selected_items).
    /// `selected_items = None`  → run_update() (full update)
    /// `selected_items = Some(v)` where v is non-empty → run_selected_update(v)
    backends: Vec<(Arc<dyn Backend>, Option<Vec<String>>)>,
}
```

#### 4.4.2 Constructor

```rust
impl UpdateOrchestrator {
    pub fn new(backends: Vec<(Arc<dyn Backend>, Option<Vec<String>>)>) -> Self {
        Self { backends }
    }
```

#### 4.4.3 Backend iteration — dispatch logic

```rust
for (backend, selected_items) in &self.backends {
    // ...
    let result = match selected_items {
        Some(items) if backend.supports_item_selection() && !items.is_empty() => {
            backend.run_selected_update(items, &runner).await
        }
        _ => backend.run_update(&runner).await,
    };
    // ...
}
```

The `any_needs_root` check already works correctly because `needs_root()` is independent of item selection.

---

### 4.5 UpdateRow UI Changes — `src/ui/update_row.rs`

#### 4.5.1 New fields

```rust
pub struct UpdateRow {
    // ... (existing fields unchanged) ...

    /// Whether the backend this row represents supports per-item selection.
    /// Set once at construction from `backend.supports_item_selection()`.
    supports_item_selection: bool,

    /// Tracks item IDs that the user has DESELECTED (excluded from update).
    /// When empty, all items are selected.
    deselected_items: Rc<RefCell<HashSet<String>>>,

    /// Tracks ALL item IDs currently loaded (from the most recent set_packages call).
    all_item_ids: Rc<RefCell<Vec<String>>>,

    /// SignalHandlerId for `skip_checkbox.connect_toggled`, stored so the
    /// handler can be blocked when we update the checkbox state programmatically
    /// to avoid reentrancy.
    skip_checkbox_signal: Rc<Cell<Option<glib::signal::SignalHandlerId>>>,

    /// Guard flag: `true` while the parent-checkbox state is being updated
    /// programmatically in response to a child-checkbox toggle. Prevents
    /// the parent's `connect_toggled` handler from running at that moment.
    updating_parent: Rc<Cell<bool>>,

    /// Callback invoked when per-item selection changes (in addition to
    /// `on_skip_changed`). Used to refresh the "Update All" button state.
    on_selection_changed: Rc<dyn Fn()>,
}
```

#### 4.5.2 Constructor changes

Add `on_selection_changed` parameter and `backend.supports_item_selection()` read:

```rust
pub fn new(
    backend: &dyn Backend,
    on_skip_changed: impl Fn() + 'static,
    on_retry: impl Fn() + 'static,
    on_selection_changed: impl Fn() + 'static,  // NEW
) -> Self
```

Store `supports_item_selection: backend.supports_item_selection()`.

Initialize new fields:
```rust
let deselected_items: Rc<RefCell<HashSet<String>>> = Rc::new(RefCell::new(HashSet::new()));
let all_item_ids: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
let updating_parent: Rc<Cell<bool>> = Rc::new(Cell::new(false));
let skip_checkbox_signal: Rc<Cell<Option<glib::signal::SignalHandlerId>>> = Rc::new(Cell::new(None));
```

The `skip_checkbox.connect_toggled` handler must be updated to handle the additional selection semantics (see §4.5.4).

#### 4.5.3 `set_packages()` — changes

When `supports_item_selection` is `true`, each package row gains a prefix `gtk::CheckButton`:

```rust
pub fn set_packages(&self, packages: &[String]) {
    // Reset deselected set and item IDs before repopulating.
    self.deselected_items.borrow_mut().clear();
    *self.all_item_ids.borrow_mut() = packages.to_vec();

    // ... (existing cache update and row-removal logic unchanged) ...

    // Reset parent checkbox to "all included" state.
    self.set_parent_checkbox_state_from_selection();

    for pkg in &packages[..display_count] {
        let pkg_row = adw::ActionRow::builder().title(pkg.as_str()).build();

        if self.supports_item_selection {
            let cb = gtk::CheckButton::builder()
                .active(true)           // checked = included in update
                .valign(gtk::Align::Center)
                .build();
            // Accessibility label
            let label = gettext("Include {} in update").replace("{}", pkg);
            cb.update_property(&[gtk::accessible::Property::Label(label.as_str())]);

            // Connect item checkbox: toggle updates deselected_items set,
            // then refreshes the parent checkbox state.
            {
                let pkg_id = pkg.clone();
                let deselected = self.deselected_items.clone();
                let all_ids = self.all_item_ids.clone();
                let skip_cb = self.skip_checkbox.clone();
                let updating_parent = self.updating_parent.clone();
                let skip_signal_slot = self.skip_checkbox_signal.clone();
                let on_selection_changed = self.on_selection_changed.clone();
                cb.connect_toggled(move |item_cb| {
                    if item_cb.is_active() {
                        deselected.borrow_mut().remove(&pkg_id);
                    } else {
                        deselected.borrow_mut().insert(pkg_id.clone());
                    }
                    // Update parent checkbox without triggering its toggled handler.
                    updating_parent.set(true);
                    let total = all_ids.borrow().len();
                    let desel_count = deselected.borrow().len();
                    if desel_count == 0 {
                        // All selected → parent shows checkmark (not skipped)
                        skip_cb.set_inconsistent(false);
                        skip_cb.set_active(false);
                    } else if desel_count < total {
                        // Partial → parent shows dash (indeterminate, not skipped)
                        skip_cb.set_inconsistent(true);
                        skip_cb.set_active(false);
                    } else {
                        // All deselected → parent shows unchecked (skipped)
                        skip_cb.set_inconsistent(false);
                        skip_cb.set_active(true);
                    }
                    updating_parent.set(false);
                    (*on_selection_changed)();
                });
            }

            pkg_row.add_prefix(&cb);
            // Make the row activatable via its checkbox (clicking the row toggles the checkbox).
            pkg_row.set_activatable_widget(Some(&cb));
        }

        self.row.add_row(&pkg_row);
        tracked.push(pkg_row);
    }
    // ... (existing "and N more" summary row logic unchanged) ...
}
```

#### 4.5.4 Parent skip_checkbox `connect_toggled` — updated semantics

The handler must be aware of `updating_parent` to avoid reentrancy, and must handle the three-state semantics:

```rust
skip_checkbox.connect_toggled(move |cb| {
    // Ignore programmatic updates from child checkbox toggles.
    if updating_parent.get() {
        return;
    }

    let supports_sel = supports_item_selection_flag;

    if supports_sel && cb.is_inconsistent() {
        // User clicked the dash (indeterminate) → select all items
        // (programmatically check all child checkboxes, clear deselected set)
        deselected_items.borrow_mut().clear();
        cb.set_inconsistent(false);
        cb.set_active(false);
        // Re-check all child checkboxes (requires access to child CheckButton refs)
        for child_cb in child_checkboxes.borrow().iter() {
            child_cb.set_active(true);
        }
        skip_flag.set(false);
        on_skip_changed();
        on_selection_changed();
        return;
    }

    let skipped = cb.is_active();
    skip_flag.set(skipped);

    if skipped && supports_sel {
        // User unchecked all → deselect all child checkboxes
        for child_cb in child_checkboxes.borrow().iter() {
            child_cb.set_active(false);
        }
        let ids = all_item_ids.borrow().clone();
        *deselected_items.borrow_mut() = ids.into_iter().collect();
    } else if !skipped && supports_sel {
        // User re-checked after full skip → re-select all
        deselected_items.borrow_mut().clear();
        for child_cb in child_checkboxes.borrow().iter() {
            child_cb.set_active(true);
        }
    }

    // ... existing status_label update logic unchanged ...
    on_skip_changed();
});
```

This requires an additional field `child_checkboxes: Rc<RefCell<Vec<gtk::CheckButton>>>` to track active item checkboxes so they can be bulk-toggled.

#### 4.5.5 New public methods

```rust
/// Returns `Some(items)` when a non-empty proper subset of packages is
/// selected, where `items` is the list of IDs to include in the update.
///
/// Returns `None` when:
/// - all packages are selected (full update — no filter needed), OR
/// - the backend does not support item selection, OR
/// - there are no packages loaded.
pub fn items_to_update(&self) -> Option<Vec<String>> {
    if !self.supports_item_selection {
        return None;
    }
    let all = self.all_item_ids.borrow();
    let desel = self.deselected_items.borrow();
    if desel.is_empty() || all.is_empty() {
        return None; // All selected or nothing loaded
    }
    if desel.len() >= all.len() {
        return None; // All deselected = backend is skipped (handled by is_skipped())
    }
    let selected: Vec<String> = all.iter()
        .filter(|id| !desel.contains(*id))
        .cloned()
        .collect();
    if selected.is_empty() { None } else { Some(selected) }
}

/// Returns `true` when some (not all) packages are deselected —
/// i.e., a selective update would run.
pub fn has_partial_selection(&self) -> bool {
    if !self.supports_item_selection {
        return false;
    }
    let all_count = self.all_item_ids.borrow().len();
    let desel_count = self.deselected_items.borrow().len();
    desel_count > 0 && desel_count < all_count
}
```

#### 4.5.6 Visual design — read-only items for non-selective backends

When `supports_item_selection` is `false`, `set_packages()` creates plain `adw::ActionRow` rows as before (no checkbox). This is the existing behavior; no change needed for non-selective backends.

For selective backends, the `gtk::CheckButton` prefix uses the existing `accent` CSS class only when the row is hovered, following standard GNOME HIG checkbox patterns.

---

### 4.6 Window Changes — `src/ui/window.rs`

#### 4.6.1 UpdateRow construction

Add the `on_selection_changed` callback. This callback should refresh the "Update All" button sensitivity in the same way `on_skip_changed` does. Reuse the existing skip-changed callback closure (both conditions that can disable the button):

```rust
let update_row = UpdateRow::new(
    backend.as_ref(),
    {
        // on_skip_changed
        let rows = rows.clone();
        let update_button = update_button.clone();
        move || { /* existing sensitivity logic */ }
    },
    on_retry_closure,
    {
        // on_selection_changed (NEW) — same re-evaluation of button sensitivity
        let rows = rows.clone();
        let update_button = update_button.clone();
        move || { /* same sensitivity logic as on_skip_changed */ }
    },
);
```

#### 4.6.2 Build backends list for "Update All"

Replace the existing `backends: Vec<Arc<dyn Backend>>` collection:

```rust
let backends: Vec<(Arc<dyn Backend>, Option<Vec<String>>)> = {
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
        .map(|b| {
            let items = rows_borrow
                .iter()
                .find(|(k, _)| *k == b.kind())
                .and_then(|(_, r)| r.items_to_update());
            (b.clone(), items)
        })
        .collect()
};

let orchestrator = UpdateOrchestrator::new(backends);
```

No other window changes are required.

---

### 4.7 State Management Summary

| Location | What is stored | Scope |
|----------|---------------|-------|
| `UpdateRow.deselected_items` | `HashSet<String>` of excluded IDs | Per-backend, UI-lifetime |
| `UpdateRow.all_item_ids` | `Vec<String>` from last `set_packages()` | Per-backend, refreshed on re-check |
| `UpdateRow.child_checkboxes` | `Vec<gtk::CheckButton>` for bulk toggle | Per-backend, refreshed on `set_packages()` |
| `UpdateOrchestrator.backends` | `Vec<(Arc<dyn Backend>, Option<Vec<String>>)>` | Transient, built at update start |

There is **no central store** for selections. State is fully encapsulated in the per-backend `UpdateRow`. When the user triggers a re-check (`run_checks()`), `set_packages()` is called again, which resets all selections to "all included". This is intentional: after a re-check the package list may have changed.

---

## 5. Per-Backend Support Matrix

| Backend | `list_available()` returns | `supports_item_selection` | Selective command | Notes |
|---------|---------------------------|--------------------------|-------------------|-------|
| APT | Package names | **true** | `apt-get install --only-upgrade -y <pkgs>` | Needs root |
| DNF | Package names | **true** | `dnf upgrade -y <pkgs>` | Needs root |
| Pacman | Package names | **false** | N/A | Partial upgrades unsupported by Arch |
| Zypper | Package names | **true** | `zypper --non-interactive update <pkgs>` | Needs root |
| Flatpak | App IDs | **true** | `flatpak update -y <app-ids>` | Unprivileged |
| Homebrew | Formula names | **true** | `brew upgrade <formulas>` | Unprivileged |
| Nix (NixOS flake) | Flake input names | **true** | `nix flake update <inputs> && nixos-rebuild switch` | Needs root; full rebuild still runs |
| Nix (NixOS channel) | `[]` | **false** | N/A | Cannot enumerate; always full rebuild |
| Nix (profile, modern) | `[]` | **false** | N/A | No dry-run |
| Nix (profile, legacy) | Package names | **false** | N/A | `nix-env -u` cannot target single packages safely |
| Determinate Nix | `["determinate-nix"]` | **false** | N/A | Single-item; no meaningful selection |
| Fwupd | Display strings | **false** | N/A | Device GUIDs not exposed in current parser |
| Plugin | Backend-defined | **false** (default) | N/A | Schema v1 lacks `select_update` command |

**Read-only display**: For backends where `supports_item_selection = false`, the existing plain `adw::ActionRow` display is unchanged. Users can still see which items are pending, but cannot deselect them.

---

## 6. Implementation Steps (Ordered)

### Step 1 — Backend trait additions (`src/backends/mod.rs`)

1. Add `supports_item_selection(&self) -> bool { false }` to the `Backend` trait with default.
2. Add `run_selected_update<'a>(&'a self, items: &'a [String], runner: &'a dyn CommandExecutor) -> Pin<...>` with default (delegates to `run_update`).
3. No changes to existing method signatures.

**Verification:** `cargo build` — all existing backends still compile.

### Step 2 — Implement `supports_item_selection` + `run_selected_update` per backend

In order of complexity (simplest first):

2a. `src/backends/homebrew.rs` — `HomebrewBackend`  
2b. `src/backends/flatpak.rs` — `FlatpakBackend`  
2c. `src/backends/os_package_manager.rs` — `AptBackend`, `DnfBackend`, `ZypperBackend`  
   - Add Zypper helper `count_zypper_upgraded()` if not yet present.  
2d. `src/backends/nix.rs` — `NixBackend` (flake case only)  

**Verification after each file:** `cargo build && cargo clippy -- -D warnings`

### Step 3 — Orchestrator changes (`src/orchestrator.rs`)

1. Change `UpdateOrchestrator.backends` type from `Vec<Arc<dyn Backend>>` to `Vec<(Arc<dyn Backend>, Option<Vec<String>>)>`.
2. Update `UpdateOrchestrator::new()` parameter type.
3. In `run_all()`, replace `backend.run_update(&runner)` with the dispatch pattern from §4.4.3.
4. `CleanupOrchestrator` is **not changed** — cleanup never uses per-item selection.

**Verification:** `cargo build` — window.rs will fail to compile until Step 5, but `cargo check -p up` is sufficient here.

### Step 4 — UpdateRow UI changes (`src/ui/update_row.rs`)

4a. Add new fields to `UpdateRow` struct:
- `supports_item_selection: bool`
- `deselected_items: Rc<RefCell<HashSet<String>>>`
- `all_item_ids: Rc<RefCell<Vec<String>>>`
- `child_checkboxes: Rc<RefCell<Vec<gtk::CheckButton>>>`
- `updating_parent: Rc<Cell<bool>>`
- `skip_checkbox_signal: Rc<Cell<Option<glib::signal::SignalHandlerId>>>`
- `on_selection_changed: Rc<dyn Fn()>`

4b. Update `UpdateRow::new()` signature to accept `on_selection_changed`.

4c. Store the `SignalHandlerId` from `skip_checkbox.connect_toggled` and hold it in `skip_checkbox_signal`.

4d. Update `skip_checkbox.connect_toggled` handler per §4.5.4.

4e. Update `set_packages()` per §4.5.3 — add per-item `gtk::CheckButton` when `supports_item_selection`.

4f. Clear `child_checkboxes` when `set_packages()` begins, repopulate alongside `pkg_rows`.

4g. Add helper method `set_parent_checkbox_state_from_selection()` that derives correct `active`/`inconsistent` state from `all_item_ids` and `deselected_items`.

4h. Add public methods `items_to_update() -> Option<Vec<String>>` and `has_partial_selection() -> bool`.

**Note:** `std::collections::HashSet` must be imported. No new crate dependency.

**Verification:** `cargo build`

### Step 5 — Window changes (`src/ui/window.rs`)

5a. Update all `UpdateRow::new(...)` call sites to pass the `on_selection_changed` closure.

5b. In the "Update All" button click handler, change the backends collection to `Vec<(Arc<dyn Backend>, Option<Vec<String>>)>` per §4.6.2.

5c. Update `UpdateOrchestrator::new(backends)` call to pass the new type.

**Verification:** `cargo build`

### Step 6 — Tests

6a. `src/backends/flatpak.rs` — add `test_flatpak_run_selected_update_with_ids()` and `test_flatpak_run_selected_update_invalid_id()`.  
6b. `src/backends/os_package_manager.rs` — add tests for APT, DNF selective update.  
6c. `src/backends/homebrew.rs` — add `test_homebrew_run_selected_update()`.  
6d. `src/backends/nix.rs` — add `test_nix_selective_flake_inputs()`.

### Step 7 — Format and lint pass

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```

---

## 7. Files Modified / Created

### Modified files

| File | Change summary |
|------|---------------|
| `src/backends/mod.rs` | +2 trait methods: `supports_item_selection`, `run_selected_update` |
| `src/backends/flatpak.rs` | +`supports_item_selection`, +`run_selected_update` |
| `src/backends/os_package_manager.rs` | +`supports_item_selection`, +`run_selected_update` for APT, DNF, Zypper; +`count_zypper_upgraded` helper if missing |
| `src/backends/homebrew.rs` | +`supports_item_selection`, +`run_selected_update` |
| `src/backends/nix.rs` | +`supports_item_selection` (flake check), +`run_selected_update` (flake inputs) |
| `src/orchestrator.rs` | `UpdateOrchestrator` type change + dispatch logic |
| `src/ui/update_row.rs` | +6 fields, updated `set_packages`, +2 public methods, `new()` signature change |
| `src/ui/window.rs` | `UpdateRow::new` call sites + backends collection type |

### New files

None. All changes fit within existing modules.

---

## 8. Risks and Mitigations

| Risk | Severity | Mitigation |
|------|----------|-----------|
| **GTK reentrancy in toggle handler**: `skip_checkbox.connect_toggled` fires while we're programmatically setting `active`/`inconsistent`, causing infinite recursion | High | Use `updating_parent: Rc<Cell<bool>>` guard flag; handler returns early when `updating_parent` is `true` |
| **Shell injection via package names**: Selective update commands interpolate user-visible strings into shell commands | High | Validate each item ID against a strict allowlist (alphanumeric + known-safe separators) before use; reject and return `BackendError` on invalid input |
| **NixOS partial flake updates leaving system inconsistent**: Updating one input (e.g., `nixpkgs`) without another (e.g., `home-manager`) can cause module version mismatch | Medium | Document risk in tooltip/UI; the rebuild proceeds regardless, and NixOS's evaluation-time type checking will catch most incompatibilities |
| **Pacman partial upgrade breakage**: End users may expect per-item selection for Pacman | Medium | `supports_item_selection = false` for Pacman; items are read-only. Consider adding a tooltip explaining why |
| **Flatpak sandbox vs. host selection**: In Flatpak sandbox, `flatpak-spawn --host flatpak update -y <id>` targets the host's user Flatpak installation | Low | Already handled by `build_flatpak_cmd`; the selective variant uses the same helper |
| **`set_packages()` re-check resets selections**: If the user deselects items and then the window auto-refreshes, selections are lost | Low | Acceptable UX tradeoff; per-check selections are transient by design. Can be revisited if user feedback demands persistence |
| **"and N more" overflow row + checkboxes**: When `packages.len() > 50`, items beyond 50 are not shown. Their checkboxes are not rendered and they are silently included | Low | They are included in `all_item_ids` but cannot be deselected. Document the 50-item cap or increase it for selective backends |
| **Orchestrator type change breaks `CleanupOrchestrator`**: `CleanupOrchestrator` and `UpdateOrchestrator` are separate; only `UpdateOrchestrator` changes | None | Verify via `cargo build` |
| **`run_selected_update` default fallback on non-selective backends**: If window passes `items = Some(v)` for a backend where `supports_item_selection = false`, the orchestrator falls back to `run_update()`, silently upgrading everything | Low | The dispatch condition checks `backend.supports_item_selection()` before calling `run_selected_update()`; non-selective backends always receive `run_update()` |

---

## Appendix A — Dependency Verification

All implementation uses existing crates already in `Cargo.toml`:

| Crate | Version in Cargo.toml | Usage |
|-------|----------------------|-------|
| `gtk4` (`gtk`) | `0.9`, features `v4_12` | `gtk::CheckButton::set_inconsistent()`, `gtk::CheckButton::is_inconsistent()` — stable in GTK 4.12 |
| `libadwaita` (`adw`) | `0.7`, features `v1_5` | `adw::ExpanderRow::add_row()`, `adw::ActionRow::set_activatable_widget()` — stable in libadwaita 1.5 |
| `glib` | `0.20` | `glib::signal::SignalHandlerId`, `block_signal`/`unblock_signal` |
| `std::collections::HashSet` | stdlib | Per-backend deselection tracking |

No new Cargo dependencies required.

---

## Appendix B — UX Behaviour Summary

**Scenario A — All items selected (default after check)**
- Parent checkbox: filled checkmark `active=false, inconsistent=false` (= "not skipped")
- All child checkboxes: checked
- "Update All" → full `run_update()`

**Scenario B — Some items deselected**
- Parent checkbox: dash `active=false, inconsistent=true` (indeterminate)
- Some child checkboxes: unchecked
- "Update All" → `run_selected_update(selected_items)`
- User can click dash → selects all → scenario A

**Scenario C — All items deselected (backend skipped)**
- Parent checkbox: empty `active=true` (same as current "skipped" state)
- All child checkboxes: unchecked
- "Update All" → backend is filtered out entirely (`is_skipped()` returns `true`)
- User can click empty → selects all → scenario A

**Scenario D — Non-selective backend (e.g. Pacman, NixOS channel)**
- Parent checkbox: same as current (skip/include only)
- Child rows: plain `adw::ActionRow` with no checkboxes (read-only display)
- "Update All" → full `run_update()` always
