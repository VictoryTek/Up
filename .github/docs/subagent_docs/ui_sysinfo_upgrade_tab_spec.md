# Specification: Move System Info to Update Tab & Conditionally Hide Upgrade Tab

**Feature Name:** `ui_sysinfo_upgrade_tab`  
**Date:** 2026-04-17  
**Author:** Research Subagent  

---

## 1. Current State Analysis

### 1.1 System Information — Where It Lives Now

System information is exclusively displayed in **`src/ui/upgrade_page.rs`** inside the `UpgradePage::build()` function.

It is rendered as an `adw::PreferencesGroup` named **"System Information"** containing three `adw::ActionRow`s:

| Row Title | Initial Subtitle | Populated By |
|---|---|---|
| Distribution | "Loading…" | `upgrade::detect_distro()` result → `info.name` |
| Current Version | "Loading…" | `upgrade::detect_distro()` result → `info.version` |
| Upgrade Available | "Loading…" or "Not supported…" | `upgrade::check_upgrade_available()` result |

An optional fourth row (`NixOS Config Type`) is added conditionally when `info.id == "nixos"`.

The distro detection (`upgrade::detect_distro()`) is called **inside `UpgradePage::build()`** using `super::spawn_background_async()` to run off the GTK thread. The result is sent back to the GTK main loop via an `async_channel` and used to:

1. Populate all three info rows above  
2. Store `DistroInfo` in `distro_info_state: Rc<RefCell<Option<upgrade::DistroInfo>>>`  
3. Enable/disable the "Run Checks" button based on `info.upgrade_supported`  
4. Conditionally show the `flake_banner` for NixOS Flake systems  

### 1.2 Update Page — Current Structure

Defined in **`src/ui/window.rs`** via `UpWindow::build_update_page()`.

Content (top to bottom):
1. `status_label` — "Detect available updates across your system."
2. `backends_group` — `adw::PreferencesGroup` titled **"Sources"** containing per-backend `UpdateRow`s
3. `log_panel` — expandable terminal output
4. `restart_banner` — revealed only when Up itself is Flatpak-updated
5. `update_button` — "Update All" button

There is **no system information section** in the update page.

### 1.3 Upgrade Page — Current Structure

Defined in **`src/ui/upgrade_page.rs`** via `UpgradePage::build()`.

Content (top to bottom):
1. `flake_banner` — NixOS Flake advisory (conditionally revealed)
2. Header label — "Upgrade your distribution to the next major version."
3. `info_group` — `adw::PreferencesGroup` titled **"System Information"** (Distro, Version, Upgrade Available, optional NixOS Config Type)
4. `prereq_group` — `adw::PreferencesGroup` titled **"Prerequisites"**
5. `log_panel` — expandable terminal output
6. Buttons — "Run Checks", "Start Upgrade"
7. `backup_check` — "I have backed up my important data"

### 1.4 Tab Registration — `src/ui/window.rs` `UpWindow::build()`

```rust
let view_stack = adw::ViewStack::new();

let (update_page, run_checks) = Self::build_update_page();
view_stack.add_titled_with_icon(&update_page, Some("update"), "Update", "...");

let upgrade_page = UpgradePage::build();
view_stack.add_titled_with_icon(&upgrade_page, Some("upgrade"), "Upgrade", "...");
```

The **return value of `add_titled_with_icon()`** — an `adw::ViewStackPage` — is currently **discarded** for both tabs. Hiding the upgrade tab requires retaining the `adw::ViewStackPage` handle.

### 1.5 Distro Detection — `src/upgrade.rs`

`upgrade::detect_distro()` parses `/etc/os-release` and returns `DistroInfo`:

```rust
pub struct DistroInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub version_id: String,
    pub upgrade_supported: bool,
}
```

The `upgrade_supported` flag is currently set to `true` only for:

```rust
let upgrade_supported = matches!(
    id.as_str(),
    "ubuntu" | "fedora" | "opensuse-leap" | "debian" | "nixos"
);
```

All other distros (Arch/Pacman, openSUSE Tumbleweed, Manjaro, Void, Gentoo, unknown, etc.) have `upgrade_supported = false`.

### 1.6 `BackendKind` and Backend Detection

`src/backends/mod.rs` defines:

```rust
pub enum BackendKind { Apt, Dnf, Pacman, Zypper, Flatpak, Homebrew, Nix }
```

`detect_backends()` detects OS backends via `which` (apt, dnf, pacman, zypper) plus Flatpak, Homebrew, and Nix. Importantly, **backends are independent of distro detection** — a Pacman backend can be detected alongside `upgrade_supported = false`.

---

## 2. Problem Definition

### 2.1 Change 1 — System Information Must Move to Update Tab

Users who never use the Upgrade tab (rolling-release distros, or users who prefer manual upgrades) never see system information about their distro. It belongs on the update tab, which all users access regularly.

System information (distro name, version) should appear **above the "Sources" section** on the update tab — applicable to every user, not just those upgrading.

### 2.2 Change 2 — Upgrade Tab Must Be Hidden for Non-Upgradeable Distros

Currently the Upgrade tab is always visible, even on Arch Linux, Manjaro, Void Linux, openSUSE Tumbleweed, or any rolling-release distro where version upgrades are meaningless. Showing this tab for unsupported distros is confusing and misleading.

The tab should be **hidden at runtime** after distro detection, only shown when `upgrade_supported = true`.

---

## 3. Distro Classification Table

| Distro Name | `os-release` ID | Package Manager | `upgrade_supported` | Shows Upgrade Tab |
|---|---|---|---|---|
| Ubuntu (22.04, 24.04 …) | `ubuntu` | APT | ✅ true | ✅ Yes |
| Debian (stable/bookworm …) | `debian` | APT | ✅ true | ✅ Yes |
| Fedora | `fedora` | DNF | ✅ true | ✅ Yes |
| openSUSE Leap | `opensuse-leap` | Zypper | ✅ true | ✅ Yes |
| NixOS (stable channels) | `nixos` | Nix | ✅ true | ✅ Yes |
| Arch Linux | `arch` | Pacman | ❌ false | ❌ Hidden |
| Manjaro | `manjaro` | Pacman | ❌ false | ❌ Hidden |
| EndeavourOS | `endeavouros` | Pacman | ❌ false | ❌ Hidden |
| Garuda Linux | `garuda` | Pacman | ❌ false | ❌ Hidden |
| openSUSE Tumbleweed | `opensuse-tumbleweed` | Zypper | ❌ false | ❌ Hidden |
| Void Linux | `void` | XBPS | ❌ false | ❌ Hidden |
| Gentoo | `gentoo` | Portage | ❌ false | ❌ Hidden |
| Unknown / unlisted | `*` | any | ❌ false | ❌ Hidden |

> **Note:** The existing `upgrade::detect_distro()` already computes `upgrade_supported` correctly via the `matches!` macro. No changes to that logic are needed. The classification is already correct.

---

## 4. Proposed Solution Architecture

### 4.1 Core Principle: Single Distro Detection, Fanned Out

Distro detection currently happens **inside `UpgradePage::build()`**. It must be lifted to **`UpWindow::build()`** so a single detection result can:

1. Populate system info on the **update page**
2. Gate the **upgrade tab visibility** on the ViewStack
3. Bootstrap the **upgrade page internal logic** (which still needs `DistroInfo`)

### 4.2 Detection Result Distribution

`UpWindow::build()` will:
1. Start a single background distro detection task
2. Receive `(DistroInfo, Option<(NixOsConfigType, String)>)` via `async_channel`
3. In the GTK main-loop callback:
   a. Populate the system info group on the update page (via `Rc<RefCell<_>>` captured closures)  
   b. Hide or show the upgrade tab via `adw::ViewStackPage::set_visible()`  
   c. Relay `DistroInfo` to the upgrade page via a dedicated channel  

### 4.3 Upgrade Page API Change

`UpgradePage::build()` signature changes from:

```rust
pub fn build() -> gtk::Box
```

to:

```rust
pub fn build() -> (gtk::Box, async_channel::Sender<UpgradePageInit>)
```

Where `UpgradePageInit` is a new type (or tuple alias):

```rust
pub struct UpgradePageInit {
    pub distro: upgrade::DistroInfo,
    pub nixos_extra: Option<(upgrade::NixOsConfigType, String)>,
}
```

The page **removes** its own `spawn_background_async` detection block and instead **listens** on the receiver end of the channel. All downstream logic (enabling "Run Checks", showing `flake_banner`, auto-triggering checks) remains intact, just triggered by the externally injected `DistroInfo`.

### 4.4 Update Page API Change

`UpWindow::build_update_page()` signature changes from:

```rust
fn build_update_page() -> (gtk::Box, Rc<dyn Fn()>)
```

to:

```rust
fn build_update_page() -> (gtk::Box, Rc<dyn Fn()>, SysInfoHandles)
```

Where `SysInfoHandles` is a small struct or tuple holding mutable references to the two info rows so the detection callback can populate them:

```rust
struct SysInfoHandles {
    distro_row: adw::ActionRow,
    version_row: adw::ActionRow,
}
```

These row references are captured from within `build_update_page()` and returned to `build()` for population.

> **Alternative:** Use `Rc<RefCell<Option<upgrade::DistroInfo>>>` shared between `build_update_page` and `build`. The simpler approach of just returning the two `adw::ActionRow` clones is preferred; they are cheap `GObject` reference-counted handles.

### 4.5 `adw::ViewStackPage` Visibility API

`adw::ViewStack::add_titled_with_icon()` returns `adw::ViewStackPage`. The Rust binding exposes:

```rust
// adw::prelude::ViewStackPageExt (from the generated GObject bindings)
upgrade_stack_page.set_visible(false);
```

This removes the tab from `AdwViewSwitcherBar` display and prevents navigation to it. The page content is not destroyed.

Confirmed by: libadwaita C API `AdwViewStackPage:visible` property, which GTK4-rs maps to `set_visible(bool)`.

### 4.6 System Info Layout on Update Tab

The new system information group is placed **between `status_label` and `backends_group`** in the update page layout:

```
┌─ Update Page ─────────────────────────────────────────┐
│  "Detect available updates across your system."        │  ← status_label (unchanged)
│                                                         │
│  ┌─ System Information ────────────────────────────┐   │  ← NEW info_group
│  │  [💻] Distribution    │ <distro name>           │   │
│  │       Current Version │ <version>               │   │
│  └─────────────────────────────────────────────────┘   │
│                                                         │
│  ┌─ Sources ───────────────────────────────────────┐   │  ← backends_group (unchanged)
│  │  APT   — Checking...  [spinner]                 │   │
│  │  ...                                            │   │
│  └─────────────────────────────────────────────────┘   │
│                                                         │
│  ▶ Terminal Output                                      │
│  [Update All]                                           │
└─────────────────────────────────────────────────────────┘
```

The "Upgrade Available" row is **not** moved to the update page. It remains conceptually tied to the upgrade flow and stays in the upgrade page's info section.

On the **upgrade page**, the `info_group` is simplified:

- Remove: Distribution row  
- Remove: Current Version row  
- Keep: Upgrade Available row  
- Keep: NixOS Config Type row (conditional)

The upgrade page's `info_group` title changes to **"Upgrade Status"** (or the group is replaced with just the upgrade-specific rows).

---

## 5. Implementation Steps

### Step 1 — Add `UpgradePageInit` struct to `upgrade.rs`

File: `src/upgrade.rs`

Add a new public struct that carries the detection result from window to upgrade page:

```rust
/// Carries all detection results the upgrade page needs to initialise.
#[derive(Debug, Clone)]
pub struct UpgradePageInit {
    pub distro: DistroInfo,
    pub nixos_extra: Option<(NixOsConfigType, String)>,
}
```

This avoids passing a tuple through the channel.

### Step 2 — Refactor `UpgradePage::build()` — Remove Internal Detection

File: `src/ui/upgrade_page.rs`

**Remove** the `spawn_background_async` block that calls `upgrade::detect_distro()` and `upgrade::detect_nixos_config_type()`.

**Change** the function signature to:

```rust
pub fn build() -> (gtk::Box, async_channel::Sender<upgrade::UpgradePageInit>)
```

Create a bounded channel `(init_tx, init_rx)` of capacity 1 inside `build()`.

**Remove** the distro row and version row from `info_group` inside upgrade page (they move to update tab). Keep only:
- `upgrade_available_row` ("Upgrade Available")
- The conditional NixOS Config Type row (still added in the channel callback)

Change `info_group` title to `"Upgrade Status"`.

Inside the existing `glib::spawn_future_local(async move { ... })` block, replace the `detect_rx.recv()` pattern with `init_rx.recv()`. The body of the callback (filling rows, enabling buttons, spawning upgrade availability check) remains **identical** except:  
- `distro_row_fill.set_subtitle(...)` is removed  
- `version_row_fill.set_subtitle(...)` is removed  

Return `(page_box, init_tx)` from `build()`.

### Step 3 — Add System Info Section to `build_update_page()`

File: `src/ui/window.rs`

**Add** to `build_update_page()`:

```rust
// System Information group (populated after background detection)
let sys_info_group = adw::PreferencesGroup::builder()
    .title("System Information")
    .build();

let distro_row = adw::ActionRow::builder()
    .title("Distribution")
    .subtitle("Loading\u{2026}")
    .build();
distro_row.add_prefix(&gtk::Image::from_icon_name("computer-symbolic"));
sys_info_group.add(&distro_row);

let version_row = adw::ActionRow::builder()
    .title("Current Version")
    .subtitle("Loading\u{2026}")
    .build();
sys_info_group.add(&version_row);
```

Insert `content_box.append(&sys_info_group)` **after** `content_box.append(&status_label)` and **before** `content_box.append(&backends_group)`.

**Change** the return type to:

```rust
fn build_update_page() -> (gtk::Box, Rc<dyn Fn()>, adw::ActionRow, adw::ActionRow)
```

Return `(page_box, run_checks, distro_row, version_row)`.

### Step 4 — Lift Detection into `UpWindow::build()`

File: `src/ui/window.rs`

**Change** `build()` to:

1. Call `build_update_page()` and receive the two extra row handles:
   ```rust
   let (update_page, run_checks, sysinfo_distro_row, sysinfo_version_row) =
       Self::build_update_page();
   ```

2. Call `UpgradePage::build()` and receive the init sender:
   ```rust
   let (upgrade_widget, upgrade_init_tx) = UpgradePage::build();
   ```

3. Add both pages to the view stack, **keeping** the `ViewStackPage` handle for the upgrade tab:
   ```rust
   view_stack.add_titled_with_icon(
       &update_page, Some("update"), "Update", "software-update-available-symbolic",
   );
   let upgrade_stack_page = view_stack.add_titled_with_icon(
       &upgrade_widget, Some("upgrade"), "Upgrade", "software-update-urgent-symbolic",
   );
   ```

4. **Before** building the window's action buttons, spawn a single background detection:
   ```rust
   {
       let (detect_tx, detect_rx) = async_channel::bounded::<(
           upgrade::DistroInfo,
           Option<(upgrade::NixOsConfigType, String)>,
       )>(1);

       super::spawn_background_async(move || async move {
           let info = upgrade::detect_distro();
           let nixos_extra = if info.id == "nixos" {
               let config_type = upgrade::detect_nixos_config_type();
               let raw_hostname = upgrade::detect_hostname();
               Some((config_type, raw_hostname))
           } else {
               None
           };
           let _ = detect_tx.send((info, nixos_extra)).await;
       });

       glib::spawn_future_local(async move {
           if let Ok((info, nixos_extra)) = detect_rx.recv().await {
               // 1. Populate update-page system info rows
               sysinfo_distro_row.set_subtitle(&info.name);
               sysinfo_version_row.set_subtitle(&info.version);

               // 2. Gate upgrade tab visibility
               upgrade_stack_page.set_visible(info.upgrade_supported);

               // 3. Forward to upgrade page
               let init = upgrade::UpgradePageInit {
                   distro: info,
                   nixos_extra,
               };
               let _ = upgrade_init_tx.send(init).await;
           }
       });
   }
   ```

### Step 5 — Update Callers in `build()`

File: `src/ui/window.rs`

The existing `let upgrade_page = UpgradePage::build();` line in `build()` must be replaced by step 4 above, and `window.rs` no longer calls `UpgradePage::build()` returning a bare `gtk::Box`.

### Step 6 — Add `UpgradePageInit` to Upgrade Page Imports

File: `src/ui/upgrade_page.rs`

Add `use crate::upgrade::UpgradePageInit;` or change the existing `use crate::upgrade` import to include the new type.

### Step 7 — Update `build_update_page` Call Site Return Destructuring

File: `src/ui/window.rs`

Any place that currently calls `let (update_page, run_checks) = Self::build_update_page()` must be updated to the 4-tuple form.

There is exactly one such call site (inside `build()`).

### Step 8 — Verify Upgrade Page Logic Completeness

File: `src/ui/upgrade_page.rs`

After the refactor, verify the `init_rx.recv()` callback in `upgrade_page.rs` still:
- Sets `distro_info_state` from `init.distro`
- Sets `nixos_config_type` from `init.nixos_extra`
- Handles the `flake_banner`
- Enables the check button
- Auto-triggers checks if `init.distro.upgrade_supported` is true

Since `UpgradePage` is only shown when `upgrade_supported = true`, the check for `upgrade_supported` before enabling the button remains as-is (it will always be true when the page is visible, but it's still correct defensive code).

---

## 6. Files to Be Modified

| File | Change Type | Summary |
|---|---|---|
| `src/upgrade.rs` | Addition | Add `UpgradePageInit` struct |
| `src/ui/upgrade_page.rs` | Refactor | Remove internal detection; accept `UpgradePageInit` via channel; return `(gtk::Box, Sender)`; remove distro/version rows from `info_group` |
| `src/ui/window.rs` | Refactor + Addition | `build_update_page()` returns sysinfo row handles; `build()` spawns single detection, gates tab visibility, fans out to upgrade page |

**No changes required to:**
- `src/backends/mod.rs`
- `src/backends/os_package_manager.rs`
- `src/app.rs`
- `src/main.rs`
- `src/runner.rs`
- `src/ui/log_panel.rs`
- `src/ui/update_row.rs`
- `src/ui/mod.rs`
- `Cargo.toml`

---

## 7. Detailed API Notes

### 7.1 `adw::ViewStackPage::set_visible`

In libadwaita-rs v0.7 (`adw` crate, feature `v1_5`), `add_titled_with_icon` is defined as:

```rust
impl ViewStack {
    pub fn add_titled_with_icon(
        &self,
        child: &impl IsA<gtk::Widget>,
        name: Option<&str>,
        title: &str,
        icon_name: &str,
    ) -> ViewStackPage
```

`ViewStackPage` implements `ViewStackPageExt`, which includes:

```rust
fn set_visible(&self, visible: bool);
fn is_visible(&self) -> bool;
```

Setting `visible(false)` on the page hides the entry from `AdwViewSwitcherBar` and prevents navigation to it. The widget itself is kept in memory; it is not destroyed.

> **Confirmed API pattern:** From the libadwaita C documentation (`AdwViewStackPage:visible` property), this is a standard GObject property exposed as `set_visible` / `get_visible` in GIR-generated bindings.

### 7.2 Async Channel for `UpgradePageInit`

Use `async_channel::bounded::<UpgradePageInit>(1)` for the init channel. A capacity of 1 is sufficient because detection fires exactly once per app startup.

The sender should be stored and used as:
```rust
let _ = upgrade_init_tx.send(init).await;
```

The upgrade page keeps the receiver in a local variable, wraps it in `glib::spawn_future_local`:
```rust
let init_rx_clone = init_rx.clone();  // if needed, or move directly
glib::spawn_future_local(async move {
    if let Ok(init) = init_rx.recv().await {
        // ... populate rows, enable button, etc.
    }
});
```

---

## 8. Risks and Mitigations

| Risk | Severity | Mitigation |
|---|---|---|
| Upgrade page receives `DistroInfo` with `upgrade_supported = true` too late (race condition) | Low | The upgrade tab is shown by default (or at build time); it is hidden after detection. A brief flash of the tab for non-upgradeable distros is acceptable since detection typically completes in < 100ms. Alternatively: start with `upgrade_stack_page.set_visible(false)` by default, then reveal for supported distros. |
| Detection off-thread races with GTK widget destruction | Very Low | Detection uses an owned `async_channel`; if receiver is dropped (e.g., window closed), `send().await` returns `Err(SendError)` and is silently discarded with `let _ = ...`. No crash. |
| `upgrade_init_tx` is held by window closure after upgrade page is rebuilt/destroyed | Very Low | There is no rebuild mechanism in this app. The init channel fires once at startup. |
| `adw::ViewStackPage::set_visible(false)` API availability in libadwaita-rs v0.7 | Low | `ViewStackPage:visible` is available since libadwaita 1.0. The project already requires `libadwaita` v0.7 (`features = ["v1_5"]`) which maps to libadwaita ≥ 1.5 — well above the requirement. |
| Upgrade page logic breaks when `upgrade_supported = false` (page is hidden but still receives no init) | Low | If the tab is hidden, `upgrade_init_tx.send()` is never called — or is called but the page simply never acts. Either is safe. Best practice: only send init when `upgrade_supported = true`. |
| `build_update_page` return type change breaks other callers | None | There is exactly one call site in `build()`. The change is self-contained within `window.rs`. |

### 8.1 Preferred Tab Default Visibility

To avoid any brief flash of the Upgrade tab on rolling-release distros, the implementation **may** choose to start with the upgrade stack page **hidden** by default, then explicitly reveal it when detection confirms `upgrade_supported = true`:

```rust
// Immediately after add_titled_with_icon:
upgrade_stack_page.set_visible(false);  // hidden by default

// In the GTK callback after detection:
if info.upgrade_supported {
    upgrade_stack_page.set_visible(true);
}
```

This is the **recommended** approach as it prevents any UI flicker.

---

## 9. Summary of Changes

| Aspect | Before | After |
|---|---|---|
| System info location | Upgrade tab "System Information" group | Update tab "System Information" group (above Sources) |
| Distro + Version rows | In upgrade page | In update page |
| "Upgrade Available" row | In upgrade page info_group | Stays in upgrade page (now in "Upgrade Status" group) |
| Distro detection call | Inside `UpgradePage::build()` | Inside `UpWindow::build()` (single shared detection) |
| Upgrade tab visibility | Always shown | Hidden when `upgrade_supported = false` |
| `UpgradePage::build()` signature | `-> gtk::Box` | `-> (gtk::Box, Sender<UpgradePageInit>)` |
| `build_update_page()` signature | `-> (gtk::Box, Rc<dyn Fn()>)` | `-> (gtk::Box, Rc<dyn Fn()>, adw::ActionRow, adw::ActionRow)` |

---

## Appendix: Code Sketch

### `src/upgrade.rs` addition

```rust
/// Carries all detection results the UpgradePage needs at initialisation time.
/// Sent once from UpWindow::build() over a bounded channel after detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradePageInit {
    pub distro: DistroInfo,
    pub nixos_extra: Option<(NixOsConfigType, String)>,
}
```

### `src/ui/upgrade_page.rs` — new signature

```rust
pub fn build() -> (gtk::Box, async_channel::Sender<upgrade::UpgradePageInit>) {
    // ...
    let (init_tx, init_rx) = async_channel::bounded::<upgrade::UpgradePageInit>(1);
    // ... build widgets ...

    // Replace old spawn_background_async + detect_rx block with:
    glib::spawn_future_local(async move {
        if let Ok(init) = init_rx.recv().await {
            let info = init.distro;
            let nixos_extra = init.nixos_extra;
            // ... same logic as before, minus distro_row/version_row population ...
        }
    });

    (page_box, init_tx)
}
```

### `src/ui/window.rs` — `build()` changes (excerpt)

```rust
let (update_page, run_checks, sysinfo_distro_row, sysinfo_version_row) =
    Self::build_update_page();

let (upgrade_widget, upgrade_init_tx) = UpgradePage::build();

view_stack.add_titled_with_icon(
    &update_page, Some("update"), "Update", "software-update-available-symbolic",
);
let upgrade_stack_page = view_stack.add_titled_with_icon(
    &upgrade_widget, Some("upgrade"), "Upgrade", "software-update-urgent-symbolic",
);

// Start hidden; reveal only after we confirm the distro supports upgrades.
upgrade_stack_page.set_visible(false);

{
    let (detect_tx, detect_rx) = async_channel::bounded::<(
        upgrade::DistroInfo,
        Option<(upgrade::NixOsConfigType, String)>,
    )>(1);

    super::spawn_background_async(move || async move {
        let info = upgrade::detect_distro();
        let nixos_extra = if info.id == "nixos" {
            let config_type = upgrade::detect_nixos_config_type();
            let raw_hostname = upgrade::detect_hostname();
            Some((config_type, raw_hostname))
        } else {
            None
        };
        let _ = detect_tx.send((info, nixos_extra)).await;
    });

    glib::spawn_future_local(async move {
        if let Ok((info, nixos_extra)) = detect_rx.recv().await {
            // Populate update-page sysinfo rows
            sysinfo_distro_row.set_subtitle(&info.name);
            sysinfo_version_row.set_subtitle(&info.version);

            // Show upgrade tab only for supported distros
            if info.upgrade_supported {
                upgrade_stack_page.set_visible(true);
            }

            // Forward init payload to upgrade page logic
            let _ = upgrade_init_tx
                .send(upgrade::UpgradePageInit {
                    distro: info,
                    nixos_extra,
                })
                .await;
        }
    });
}
```
