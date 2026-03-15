# Specification: Desktop Icon & Prerequisite Checks Fixes

**Feature Name:** `icon_and_prereq_fixes`  
**Date:** 2026-03-15  
**Status:** Draft  

---

## Current State Analysis

### Bug 1: Desktop icon not showing the app logo

**Files examined:**
- `data/io.github.up.desktop` — `Icon=io.github.up` (correct freedesktop naming)
- `meson.build` — Correctly installs SVG and PNGs into `$datadir/icons/hicolor/…/apps/`
- `src/app.rs` — No icon theme search path configuration
- `src/main.rs` — No icon setup

**Icon files present:**

| Path | Exists |
|------|--------|
| `data/icons/hicolor/scalable/apps/io.github.up.svg` | ✅ |
| `data/icons/hicolor/256x256/apps/io.github.up.png` | ✅ |
| `data/icons/hicolor/128x128/apps/io.github.up.png` | ❌ (empty dir) |
| `data/icons/hicolor/48x48/apps/io.github.up.png` | ❌ (empty dir) |

**Root cause:** The application never registers its icon directory with the GTK icon theme system. When installed via Meson (`meson install`), icons are placed into system-standard directories (e.g. `/usr/share/icons/hicolor/…`) and the icon cache is updated via `gnome.post_install()` — so the icon works after a full install. However:

1. **During development (`cargo run`):** Icons are not in any system icon theme directory. GTK4 searches only system theme paths by default. The app's `data/icons/` tree is never added as a search path, so `Icon=io.github.up` in the `.desktop` file and the window icon both fail to resolve.
2. **No `set_default_icon_name` call:** The app never calls `gtk::Window::set_default_icon_name("io.github.up")`, so even when the icon IS findable, it won't appear as the window/taskbar icon.
3. **Missing PNG sizes:** 48×48 and 128×128 directories are empty. While `meson.build` guards these with `fs.exists()`, many desktop environments prefer specific raster sizes and may fall back poorly if only scalable SVG and 256×256 PNG are available.

---

### Bug 2: Upgrade prerequisites all showing "Not Checked"

**Files examined:**
- `src/ui/upgrade_page.rs` — UI construction and button wiring
- `src/upgrade.rs` — `run_prerequisite_checks()`, `CheckResult` struct, individual check functions

**Root cause:** The prerequisite check rows are created with subtitle `"Not checked"` (line ~143 of `upgrade_page.rs`) and **are never automatically populated**. The checks only execute when the user manually clicks the **"Run Checks"** button. There is no auto-trigger on page load, page visibility, or application startup.

When the user clicks "Run Checks", the check flow works correctly:
1. A background thread calls `upgrade::run_prerequisite_checks()` 
2. Results are serialized as JSON and sent over an `async_channel`
3. The async receiver parses results and updates each row's subtitle

However, there are **secondary issues** even after clicking:

1. **Status icons never update:** Each prerequisite row has a suffix icon set to `"emblem-important-symbolic"` at creation time. When results arrive, only the subtitle text is updated — the icon is never changed to reflect pass (✅) or fail (❌) status. The suffix `gtk::Image` is created as a local variable inside the loop and is not stored for later mutation.
2. **No visual distinction between pass/fail:** Without icon updates, the user cannot quickly scan the list to see which prerequisites passed and which failed.

**Channel logic is correct:** The `drop(tx)` after thread spawn is valid — `tx_clone` (moved into the thread) keeps the channel open. The unbounded channel buffers all messages, so even if the thread completes before the receiver loop starts awaiting, messages are preserved.

---

## Problem Definition

### Bug 1: Icon Resolution Failure
The GTK4 icon theme system cannot locate `io.github.up` because:
- No search path is added for the source tree's `data/icons/` directory
- `set_default_icon_name` is never called
- Development runs (`cargo run`) bypass Meson's icon installation

### Bug 2: Prerequisites Not Auto-Checked
The upgrade page displays stale "Not checked" status because:
- Checks require manual button click — no automatic trigger
- Suffix status icons are never updated after checks complete
- The suffix `gtk::Image` widgets are not retained for later mutation

---

## Proposed Solution

### Bug 1 Fix: Icon Theme Search Path + Default Icon Name

**File: `src/app.rs`**

In `on_activate`, before creating the window:

1. Add the project's `data/icons` directory to the GTK icon theme search path. Use a relative path so it works from the project root during `cargo run`, and include an absolute fallback for installed locations.
2. Call `gtk::Window::set_default_icon_name("io.github.up")` so the window icon appears in the taskbar/title bar.

**Exact changes to `src/app.rs`:**

```rust
fn on_activate(app: &adw::Application) {
    // Add icon search path for development (cargo run from project root)
    if let Some(display) = gtk::gdk::Display::default() {
        let theme = gtk::IconTheme::for_display(&display);
        theme.add_search_path("data/icons");
    }

    gtk::Window::set_default_icon_name("io.github.up");

    let window = UpWindow::new(app);
    window.present();
}
```

**Rationale:**
- `theme.add_search_path("data/icons")` tells GTK to also look in `data/icons/hicolor/…/apps/` for icons. This mirrors the freedesktop icon theme directory structure that already exists in the repository.
- `set_default_icon_name` ensures the window/taskbar icon is set even when the system icon cache hasn't been updated.
- When installed via Meson, system paths already contain the icons, so this additional path is harmless (it just adds a redundant search location).

**No changes needed to:**
- `data/io.github.up.desktop` — `Icon=io.github.up` is already correct
- `meson.build` — install rules are already correct
- Icon filenames — already follow `io.github.up.{svg,png}` convention

**Note on missing PNGs:** The empty `48x48` and `128x128` directories are not a blocking issue. The SVG in `scalable/` and PNG in `256x256/` provide adequate coverage. Generating additional PNG sizes is optional and can be done separately.

---

### Bug 2 Fix: Auto-Trigger Checks + Update Status Icons

**File: `src/ui/upgrade_page.rs`**

Two changes:

#### Change A: Store suffix icons for later mutation

Currently, the suffix icon is created as a local variable inside a loop and cannot be accessed later:

```rust
// CURRENT (broken — icon not stored)
let status_icon = gtk::Image::from_icon_name("emblem-important-symbolic");
row.add_suffix(&status_icon);
```

**Fix:** Create a parallel `Vec` of suffix icons alongside the check rows, wrapped in `Rc<RefCell<…>>` for shared ownership:

```rust
let check_icons: Rc<RefCell<Vec<gtk::Image>>> = Rc::new(RefCell::new(Vec::new()));
for (label, icon) in &checks {
    let row = adw::ActionRow::builder()
        .title(*label)
        .subtitle("Checking...")
        .build();
    row.add_prefix(&gtk::Image::from_icon_name(*icon));

    let status_icon = gtk::Image::from_icon_name("content-loading-symbolic");
    row.add_suffix(&status_icon);

    prereq_group.add(&row);
    check_rows.borrow_mut().push(row);
    check_icons.borrow_mut().push(status_icon);
}
```

#### Change B: Extract check logic into a reusable closure and auto-trigger

Extract the check-running logic from the button click handler into a function/closure that can be called both:
1. Automatically when the page is built (for supported distros)
2. Manually when the user clicks "Run Checks"

When results arrive, update both the subtitle AND the suffix icon:

```rust
// In the results handler:
if let Ok(results) = serde_json::from_str::<Vec<upgrade::CheckResult>>(json) {
    let rows = check_rows_ref.borrow();
    let icons = check_icons_ref.borrow();
    for (i, result) in results.iter().enumerate() {
        if let Some(row) = rows.get(i) {
            row.set_subtitle(&result.message);
        }
        if let Some(icon) = icons.get(i) {
            if result.passed {
                icon.set_icon_name(Some("emblem-ok-symbolic"));
            } else {
                icon.set_icon_name(Some("dialog-error-symbolic"));
                all_passed = false;
            }
        }
    }
}
```

#### Change C: Auto-trigger checks on page build

After wiring up the button handlers, if the distro supports upgrades, immediately simulate a button click or call the check logic directly:

```rust
// Auto-trigger checks for supported distros
if distro_info.upgrade_supported {
    check_button.emit_clicked();
}
```

This triggers the same code path as the manual button click, including disabling the button during checks and re-enabling it when done.

---

## Implementation Steps

### Step 1: Fix icon resolution (`src/app.rs`)
1. Add `use gtk::prelude::*;` if not already present (it is)
2. In `on_activate`, before `UpWindow::new(app)`:
   - Get the default display and its icon theme
   - Call `theme.add_search_path("data/icons")`
   - Call `gtk::Window::set_default_icon_name("io.github.up")`

### Step 2: Store status icons (`src/ui/upgrade_page.rs`)
1. Create `check_icons: Rc<RefCell<Vec<gtk::Image>>>` alongside `check_rows`
2. In the check-row creation loop, push each suffix icon into `check_icons`
3. Change initial subtitle from `"Not checked"` to `"Checking..."` (since auto-check will run)
4. Change initial icon from `"emblem-important-symbolic"` to `"content-loading-symbolic"`

### Step 3: Update result handler to modify icons (`src/ui/upgrade_page.rs`)
1. Clone `check_icons` into the button click closure
2. In the `__RESULTS__` handler, iterate through results and update both:
   - `row.set_subtitle(&result.message)`
   - `icon.set_icon_name(Some("emblem-ok-symbolic"))` for pass
   - `icon.set_icon_name(Some("dialog-error-symbolic"))` for fail

### Step 4: Auto-trigger checks (`src/ui/upgrade_page.rs`)
1. After all button handlers are wired up, call `check_button.emit_clicked()` if `distro_info.upgrade_supported` is true
2. If distro is not supported, set subtitle to `"N/A"` and icon to `"emblem-important-symbolic"`

---

## Files to Modify

| File | Changes |
|------|---------|
| `src/app.rs` | Add icon theme search path; add `set_default_icon_name` call in `on_activate` |
| `src/ui/upgrade_page.rs` | Store suffix icons in `Rc<RefCell<Vec<gtk::Image>>>`; update icons on result; auto-trigger checks; change initial subtitle/icon |

No new files need to be created. No dependency changes required.

---

## Dependencies

No new crates or external dependencies are needed. All required APIs are already available:
- `gtk::IconTheme::for_display()` and `add_search_path()` — from `gtk4` crate (already in `Cargo.toml`)
- `gtk::Window::set_default_icon_name()` — from `gtk4` crate
- `gtk::Image::set_icon_name()` — from `gtk4` crate
- `gtk::Button::emit_clicked()` — from `gtk4` crate

---

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| `data/icons` relative path doesn't resolve when binary is run from a different working directory | Medium | Low | When installed via Meson, system icon paths are used. The relative path only matters for development. The path is relative to CWD, which is typically the project root when using `cargo run`. |
| `content-loading-symbolic` icon not available on all icon themes | Low | Low | It's a standard GNOME/freedesktop symbolic icon. Fallback: use `"emblem-synchronizing-symbolic"` or omit the loading icon and start with no suffix icon, adding it when results arrive. |
| Auto-triggering checks on page build may cause a brief UI delay | Low | Low | Checks run on a background thread via `std::thread::spawn`, so the UI remains responsive. The async channel pattern ensures non-blocking UI updates. |
| `emit_clicked()` fires before button handler is connected | Low | High | The `connect_clicked` handler is wired BEFORE `emit_clicked()` is called, so the handler is guaranteed to be registered. Verify ordering in implementation. |
| Multiple `connect_toggled` closures on backup checkbox when "Run Checks" clicked multiple times | Medium | Medium | The current code adds a new `connect_toggled` handler every time checks pass. This should be refactored: connect the toggled handler once outside the check loop, and use a shared `Rc<Cell<bool>>` flag to track whether all checks passed. |

---

## Additional Observations

1. **Backup checkbox handler accumulation:** Each successful "Run Checks" call adds a NEW `connect_toggled` closure to the backup checkbox (line ~237 of `upgrade_page.rs`). After N successful check runs, there will be N redundant closures. This should be fixed by connecting the toggled handler once during page construction and using a shared boolean flag updated by the check results.

2. **Missing PNG rasters:** The `48x48` and `128x128` directories are empty. While not blocking, generating these PNGs from the SVG would improve icon rendering on desktop environments that prefer specific raster sizes. This is out of scope for this fix.

3. **No `app.connect_startup` usage:** The icon theme path could alternatively be set in `connect_startup` instead of `connect_activate`. However, `connect_activate` is called at least once and the display is guaranteed to be available at that point, making it the simpler choice.
