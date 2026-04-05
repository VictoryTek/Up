# Specification: NixOS Pre-Update Check + Header Bar About Menu

**Feature set:** `nixos_check_and_about_menu`  
**Date:** 2026-04-04  
**Status:** Ready for Implementation  

---

## 1. Current State Analysis

### 1.1 NixOS Backend (`src/backends/nix.rs`)

The `NixBackend` currently implements the `Backend` trait as follows:

| Method | Current behaviour |
|---|---|
| `run_update` | Flake: `pkexec nix flake update && nixos-rebuild switch --flake /etc/nixos#<attr>`. Legacy: `pkexec nix-channel --update && nixos-rebuild switch` |
| `count_available` | NixOS path always returns `Err("Run Update All to check")` |
| `list_available` | NixOS path always returns `Ok(vec![])` |

Because `count_available` returns `Err(…)` for NixOS, the check loop in `window.rs` calls `row.set_status_unknown(&msg)` and does **not** add anything to `total_available`. This means the Update All button is **never enabled** if NixOS is the only or only-remaining backend with updates.

### 1.2 Window Header Bar (`src/ui/window.rs`)

The `UpWindow::build` method constructs an `adw::HeaderBar` manually and places a single refresh button on the **start** side:

```rust
let header = adw::HeaderBar::new();
let refresh_button = gtk::Button::builder()
    .icon_name("view-refresh-symbolic")
    .tooltip_text("Check for updates")
    .build();
header.pack_start(&refresh_button);
```

There is no end-side button, no application menu, and no About dialog.

### 1.3 Check / Update All Gate Logic (`src/ui/window.rs` – `build_update_page`)

The `run_checks` closure:
1. Sets `pending_checks = n` (total backends).
2. For each backend spawns a future that calls `count_available()` and `list_available()`.
3. On `Ok(count)`: calls `row.set_status_available(count)` and adds count to `total_available`.
4. On `Err(msg)`: calls `row.set_status_unknown(&msg)` — count is **not** added to `total_available`.
5. When `pending_checks` reaches 0: if `total_available > 0` enables Update All button; else shows "Everything is up to date."

The button-enable logic is already correct — the only fix needed is for NixOS to return `Ok(N)` instead of `Err(…)`.

### 1.4 Cargo Dependencies

```toml
adw = { version = "0.7", package = "libadwaita", features = ["v1_5"] }
gtk = { version = "0.9", package = "gtk4", features = ["v4_12"] }
gio = "0.20"
serde_json = "1"
```

`v1_5` is already enabled, making `adw::AboutDialog` (added in libadwaita 1.5) available. No new crate dependencies are required.

---

## 2. Problem Definition

### Feature 1 – NixOS Pre-Update Check

**Problem:** The NixOS backend cannot currently inform the user whether updates are available before they click "Update All". The Update All button is therefore never driven by NixOS data, and the row always shows a non-actionable "Run Update All to check" message.

**Goal:** Implement `count_available()` and `list_available()` for the NixOS flake and legacy-channel paths so that:
- The check phase fetches upstream state without making any persistent changes.
- The number of changed flake inputs (or available nix-env upgrades) is reported accurately.
- The row shows e.g. "3 available" with the changed input names listed as expandable sub-rows.
- The Update All button becomes active exactly when NixOS (or any other backend) has updates.

### Feature 2 – Header Bar Menu & About Dialog

**Problem:** There is no way for users to view application metadata (version, licence, source link). GNOME HIG recommends an application menu (three-dot `open-menu-symbolic` button) in the header bar end slot.

**Goal:** Add a `gtk::MenuButton` to the end of the `AdwHeaderBar`, backed by a `gio::Menu`, with one item — "About Up" — that opens an `adw::AboutDialog`.

---

## 3. Feature 1 Architecture – NixOS Pre-Update Check

### 3.1 Conceptual Approach

`nix flake update` in **Nix ≥ 2.19** (NixOS 24.05+) supports a `--dry-run` flag. When passed, Nix computes what the lock file would look like after the update and prints the diff to stderr **without** writing `flake.lock`. Output format:

```
• Updated input 'nixpkgs':
    'github:NixOS/nixpkgs/abc123' (2024-01-01)
  → 'github:NixOS/nixpkgs/def456' (2024-01-15)
• Updated input 'home-manager':
    'github:nix-community/home-manager/old' (2024-01-01)
  → 'github:nix-community/home-manager/new' (2024-01-15)
```

When everything is already at the latest commit, no `• Updated` lines are emitted.

For **Nix < 2.19** (no `--dry-run`), the fallback is a **temporary-directory copy**: copy `flake.nix` + `flake.lock` to `/tmp/up-nix-check-<uuid>/`, run `nix flake update` there (no root required since it is a temp dir), compare old vs new `flake.lock` in memory with `serde_json`, count and name the changed inputs, then clean up.

> **Why not `nix flake metadata --json`?**  
> `nix flake metadata --json` shows the *currently locked* inputs but does not reveal what is available upstream — it cannot determine staleness without a network fetch.

> **Why not `nvd` or `nix store diff-closures`?**  
> `nvd` compares two store *closures*, not pre-flight available updates. It is a post-update diff tool and is not installed by default. `nix store diff-closures` similarly requires two existing store paths. Neither is appropriate for a pre-update check.

### 3.2 NixOS Flake Check – Primary Method (`--dry-run`, Nix ≥ 2.19)

**Command:**
```
nix --extra-experimental-features 'nix-command flakes' \
    flake update --dry-run --flake /etc/nixos
```
Both stdout and stderr must be captured and concatenated. Nix writes the "Updated input" lines to **stderr**.

**Parsing `count_available`:**
```rust
let count = output
    .lines()
    .filter(|l| l.trim_start().starts_with("• Updated input '"))
    .count();
```

**Parsing `list_available`:**
```rust
let list: Vec<String> = output
    .lines()
    .filter(|l| l.trim_start().starts_with("• Updated input '"))
    .filter_map(|l| {
        // Line: "• Updated input 'nixpkgs':"
        l.split('\'').nth(1).map(|s| s.to_string())
    })
    .collect();
```

**Detecting `--dry-run` support:** If the command exits with a non-zero code **and** the output contains "unrecognised flag" or "unknown option", fall back to the temp-dir method (§3.3). Otherwise treat non-zero exit as an error.

### 3.3 NixOS Flake Check – Fallback Method (Temp Dir, All Nix Versions)

**Steps:**
1. Create a temporary directory using `std::env::temp_dir()` + a unique suffix (e.g. `up-nix-check-<timestamp>`).
2. Copy `/etc/nixos/flake.nix` and `/etc/nixos/flake.lock` into the temp dir. (Both files are world-readable on NixOS; no elevated privileges required.)
3. Read and parse the **original** `flake.lock` with `serde_json` into a `serde_json::Value`.
4. Run `nix --extra-experimental-features 'nix-command flakes' flake update <tempdir>` (unprivileged, writes to temp dir only).
5. Read and parse the **updated** `flake.lock` from the temp dir.
6. Compare `nodes` entries: for each key `k` in the new lock that exists in the old lock, check whether `locked.rev` (or `locked.lastModified` for tarballs) changed.
7. Collect the names of changed inputs.
8. Delete the temp dir.
9. Return `Ok((count, names))`.

**Flake.lock JSON structure (relevant excerpt):**
```json
{
  "nodes": {
    "nixpkgs": {
      "locked": {
        "rev": "abc123",
        "lastModified": 1704067200,
        "narHash": "sha256-..."
      }
    }
  },
  "root": "root"
}
```

**Comparison logic:**
```rust
fn compare_lock_nodes(
    old: &serde_json::Value,
    new: &serde_json::Value,
) -> Vec<String> {
    let old_nodes = old["nodes"].as_object();
    let new_nodes = new["nodes"].as_object();
    let mut changed = Vec::new();
    if let (Some(old_map), Some(new_map)) = (old_nodes, new_nodes) {
        for (name, new_val) in new_map {
            if name == "root" { continue; }
            if let Some(old_val) = old_map.get(name) {
                let old_rev = &old_val["locked"]["rev"];
                let new_rev = &new_val["locked"]["rev"];
                let old_mod = &old_val["locked"]["lastModified"];
                let new_mod = &new_val["locked"]["lastModified"];
                if old_rev != new_rev || old_mod != new_mod {
                    changed.push(name.clone());
                }
            } else {
                // New input added
                changed.push(name.clone());
            }
        }
    }
    changed
}
```

### 3.4 NixOS Legacy-Channel Check

For non-flake NixOS the existing `nix-env -u --dry-run` approach (already used for non-NixOS Nix paths) is reused:

```rust
// Legacy channels: use nix-env --dry-run (stderr contains "upgrading ..." lines)
let out = tokio::process::Command::new("nix-env")
    .args(["-u", "--dry-run"])
    .output()
    .await
    .map_err(|e| e.to_string())?;
let text = String::from_utf8_lossy(&out.stderr);
let packages: Vec<String> = text
    .lines()
    .filter(|l| l.contains("upgrading"))
    .filter_map(|l| l.split('\'').nth(1).map(|s| s.to_string()))
    .collect();
Ok(packages.len()) // for count_available
// / Ok(packages)   // for list_available
```

This already works correctly. The only change needed is wiring it into the NixOS branch of `count_available` and `list_available` (currently the NixOS branch short-circuits to `Err`/`Ok(vec![])`).

### 3.5 Shared Helper for Flake Check

To avoid duplicating the dry-run + fallback logic between `count_available` and `list_available`, introduce a private async helper:

```rust
/// Returns (count, list_of_input_names) for a flake-based NixOS update check.
/// First tries --dry-run (Nix ≥ 2.19), fallback to temp-dir comparison.
async fn check_flake_updates() -> Result<(usize, Vec<String>), String> { ... }
```

Both `count_available` and `list_available` call this helper and return the relevant part.

### 3.6 Update All Button Logic — No Changes Required

The existing gating logic in `window.rs` already works as designed:
- `Ok(count)` → adds `count` to `total_available`; button enabled when total > 0.
- `Err(msg)` → `set_status_unknown`; count not added.

The only change is making the NixOS backend return `Ok(N)` instead of `Err(…)`. No changes to `window.rs` are needed for Feature 1.

---

## 4. Feature 2 Architecture – Header Bar Menu & About Dialog

### 4.1 `adw::AboutDialog` vs `adw::AboutWindow`

| Widget | Introduced | Status in libadwaita 1.5 | Rust binding |
|---|---|---|---|
| `AdwAboutWindow` | libadwaita 1.2 | Deprecated (as of 1.6) | `adw::AboutWindow` |
| `AdwAboutDialog` | libadwaita 1.5 | **Current / recommended** | `adw::AboutDialog` |

`AdwAboutDialog` has **identical API** to `AdwAboutWindow` per the upstream migration guide. Since `Cargo.toml` already specifies `features = ["v1_5"]`, `adw::AboutDialog` is unconditionally available; no `#[cfg]` guard is needed.

### 4.2 `AdwAboutDialog` Fields

| Builder method | Value |
|---|---|
| `.application_name("Up")` | App display name |
| `.version(env!("CARGO_PKG_VERSION"))` | Read from Cargo.toml at compile time |
| `.developer_name("Up Contributors")` | Author attribution |
| `.license_type(gtk::License::Gpl30)` | GPL-3.0-or-later |
| `.website("https://github.com/user/up")` | Project homepage/repository |
| `.issue_url("https://github.com/user/up/issues")` | Issue tracker |
| `.application_icon("io.github.up")` | The app icon (already registered in GTK icon theme) |

> **Note:** The `website` and `issue_url` values must match the `repository` field in `Cargo.toml` (`"https://github.com/user/up"`). The `application_icon` must match the icon registered in `on_activate` in `app.rs` (`gtk::Window::set_default_icon_name("io.github.up")`).

### 4.3 Menu and MenuButton

```rust
// Build gio::Menu with one item targeting a window-level action
let menu = gio::Menu::new();
menu.append(Some("About Up"), Some("win.about"));

// Create the MenuButton showing the standard "open-menu-symbolic" icon
let menu_button = gtk::MenuButton::builder()
    .icon_name("open-menu-symbolic")
    .tooltip_text("Main Menu")
    .menu_model(&menu)
    .build();

// Place on the end side of the header bar (left of the window controls)
header.pack_end(&menu_button);
```

The icon name `"open-menu-symbolic"` is the GNOME standard for three-dot vertical menus ("hamburger/kebab"). It is part of the `hicolor` icon theme and available on all GNOME systems.

### 4.4 GAction Registration

The action must be registered as a **window action** using `window.add_action()`. The prefix `"win."` in the menu model string maps to this registration.

```rust
// window: adw::ApplicationWindow (already constructed earlier in build())
let window_for_about = window.clone(); // adw::ApplicationWindow implements Clone (GObject)
let action_about = gio::SimpleAction::new("about", None);
action_about.connect_activate(move |_, _| {
    let dialog = adw::AboutDialog::builder()
        .application_name("Up")
        .version(env!("CARGO_PKG_VERSION"))
        .developer_name("Up Contributors")
        .license_type(gtk::License::Gpl30)
        .website("https://github.com/user/up")
        .issue_url("https://github.com/user/up/issues")
        .application_icon("io.github.up")
        .build();
    dialog.present(Some(&window_for_about));
});
window.add_action(&action_about);
```

### 4.5 Required Imports

The existing `use adw::prelude::*;` import transitively re-exports `gio::prelude::*` (including `ActionMapExt` for `add_action`). The `gio` crate itself is already a direct project dependency, so `gio::Menu`, `gio::SimpleAction` are available without any new `use` statement beyond than what is already present, as long as `use gio;` is added or `gio::` is used fully-qualified.

In `window.rs`, add:
```rust
use gio::{Menu, SimpleAction};
```
(or use fully-qualified `gio::Menu::new()` syntax — either is fine.)

---

## 5. Implementation Steps

### Step 1 — `src/backends/nix.rs`

1. **Add private async helper `check_flake_updates()`** above the `NixBackend` impl block. The function:
   - Constructs the `--dry-run` command string.
   - Runs it via `tokio::process::Command` (not `CommandRunner` — this is a silent background check, not streamed to the log panel).
   - On success, parses `stdout + stderr` for `"• Updated input '"` lines and returns `Ok((count, names))`.
   - On failure where the error text indicates an unrecognised flag, invokes the temp-dir fallback sub-helper.
   - On other failures, returns `Err(message)`.

2. **Add private async helper `check_flake_updates_tempdir()`**: implements the temp-dir copy + parse approach (§3.3).

3. **Override `count_available()`** in `NixBackend`:
   - Replace the NixOS branch `Err("Run Update All to check")` with a call to `check_flake_updates()` (flake) or the `nix-env --dry-run` path (legacy).
   - Return `Ok(count)`.

4. **Override `list_available()`** in `NixBackend`:
   - Replace the NixOS branch `Ok(Vec::new())` with a call to `check_flake_updates()` (flake) or the `nix-env --dry-run` path (legacy).
   - Return `Ok(names)`.

5. **No changes** to `run_update()`, `is_nixos()`, `is_nixos_flake()`, `validate_flake_attr()`, `resolve_nixos_flake_attr()`, or `count_nix_store_operations()`.

### Step 2 — `src/ui/window.rs`

1. **Add import** at the top of the file (if not already transitively available):
   ```rust
   use gio::{Menu, SimpleAction};
   ```
   (Check whether `gio` is already referenced — it may be available via `adw::prelude::*`. If not, add `use gio;` or fully-qualified references.)

2. **After constructing the `window`** (line ~18 in the current file), clone it for the about-action closure:
   ```rust
   let window_for_about = window.clone();
   ```

3. **After `header.pack_start(&refresh_button)`**, add the menu button wiring (§4.3 and §4.4 code exactly as specified above).

4. **Register the action on `window`** before `window.set_content(...)`.

5. No changes elsewhere in `window.rs`.

### Step 3 — No other files require changes

- `src/backends/mod.rs`: The default implementations for `count_available` and `list_available` are already present and used as the fallback. NixBackend overrides them — no trait changes needed.
- `Cargo.toml`: No new dependencies.
- `src/ui/update_row.rs`: Already has `set_status_available`, `set_status_unknown`, and `set_packages` — no changes needed.
- `src/app.rs`: No changes.

---

## 6. Dependencies

### Crate Dependencies — No New Additions Required

| Crate | Already present | Usage in this feature |
|---|---|---|
| `serde_json = "1"` | ✅ | Parse `flake.lock` JSON in temp-dir fallback |
| `tokio` with `process` feature | ✅ | Async child-process execution in `check_flake_updates` |
| `adw = "0.7"` with `v1_5` | ✅ | `adw::AboutDialog` |
| `gio = "0.20"` | ✅ | `gio::Menu`, `gio::SimpleAction` |

### System / Runtime Requirements

| Requirement | Notes |
|---|---|
| `nix` binary in PATH | Already required by `is_available()` |
| Nix experimental features: `nix-command flakes` | Already used in `run_update` |
| `/etc/nixos/flake.nix`, `/etc/nixos/flake.lock` world-readable | Standard on NixOS; no privilege escalation needed for check |
| `nix` version ≥ 2.19 for `--dry-run` | Recommended but not mandatory; temp-dir fallback handles older versions |
| Network access during check | Required to fetch latest input revisions; shown to user via log output |

---

## 7. Risks and Mitigations

| Risk | Severity | Mitigation |
|---|---|---|
| `nix flake update --dry-run` not available on older NixOS | Medium | Implement temp-dir fallback automatically detected by error message inspection |
| `/etc/nixos/flake.nix` has complex local imports causing `nix flake update` on a temp copy to fail | Low | `nix flake update` only reads the `inputs` block, not `outputs` or module imports; in practice this works even if `./configuration.nix` is absent from the temp dir |
| Temp-dir check takes minutes on slow internet | Low | This runs in the background thread pool (same as other backends); the UI shows "Checking…" and the user can proceed with other backends first |
| Temp-dir copy fails (e.g. `/etc/nixos/flake.lock` unreadable) | Low | Return `Err(message)` which displays `set_status_unknown(msg)` — graceful degradation |
| `adw::AboutDialog` API not available at runtime (user running libadwaita < 1.5) | Low | The `v1_5` Cargo feature gates compilation; if the .so version is too old the app won't start at all, which is an existing deployment issue not introduced by this feature |
| `gio::SimpleAction` / `gio::Menu` requires `gio` in scope | Low | `gio` is already a direct dependency; adding one import line resolves this |
| Window reference in about-action closure causes reference cycle / prevents GC | Low | `adw::ApplicationWindow` is a GObject; the reference held in the closure is a strong ref but the window is always kept alive by the application anyway; no cycle introduced |

---

## 8. Verification Criteria

### Feature 1
- [ ] On a NixOS flake system with available updates, the Nix row shows "N available" after the check, where N ≥ 1.
- [ ] The expandable row lists the names of changed flake inputs (e.g. "nixpkgs", "home-manager").
- [ ] The Update All button becomes **active** when Nix (or any other backend) returns N > 0.
- [ ] On a fully up-to-date NixOS system, the Nix row shows "Up to date" and the button remains disabled (if all other backends are also up to date).
- [ ] On older Nix (no `--dry-run`), the temp-dir fallback is triggered and produces equivalent results.
- [ ] `cargo clippy -- -D warnings` and `cargo fmt --check` pass with no new warnings.

### Feature 2
- [ ] The header bar shows a three-dot `open-menu-symbolic` button on the end side (left of window controls).
- [ ] Clicking the button opens a dropdown with "About Up".
- [ ] Clicking "About Up" opens an `adw::AboutDialog` showing: app name "Up", version from `CARGO_PKG_VERSION`, developer name, GPL-3.0 licence, website link, issue URL.
- [ ] The dialog closes normally when the user clicks the X button.
- [ ] The refresh button on the start side is unaffected.
- [ ] `cargo build` succeeds without errors.

---

## 9. Source References

1. Nix manual — `nix flake update`: https://nixos.org/manual/nix/stable/command-ref/new-cli/nix3-flake-update.html  
2. Nix `--dry-run` PR/changelog (Nix 2.19 release notes): https://nixos.org/manual/nix/stable/release-notes/  
3. `flake.lock` JSON format (Nix source / NixOS wiki): https://nixos.wiki/wiki/Flakes  
4. libadwaita `AdwAboutDialog` API reference (1.5+): https://gnome.pages.gitlab.gnome.org/libadwaita/doc/1-latest/class.AboutDialog.html  
5. libadwaita migration guide — AdwAboutWindow → AdwAboutDialog: https://gnome.pages.gitlab.gnome.org/libadwaita/doc/1-latest/migrating-to-adaptive-dialogs.html  
6. gtk4-rs book — MenuButton + HeaderBar + gio::Menu pattern: https://gtk-rs.org/gtk4-rs/stable/latest/book/  
7. gtk4-rs book — Actions with `gio::SimpleAction` + `add_action_entries`: https://gtk-rs.org/gtk4-rs/stable/latest/book/actions.html  
8. libadwaita-rs API docs (0.7) — `adw::AboutDialog`: https://gtk-rs.org/gtk4-rs/stable/latest/docs/libadwaita/  
9. GNOME HIG — Application Menus: https://developer.gnome.org/hig/patterns/controls/menus.html  
