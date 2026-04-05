# NixOS Upgrade Tab Improvements — Specification

**Feature:** NixOS Upgrade Tab — Flake Awareness & Proper Channel Upgrade  
**Project:** Up — GTK4/libadwaita Linux system updater (Rust)  
**Spec Author:** Research Subagent  
**Date:** 2026-04-04  

---

## 1. Current State Analysis

### 1.1 Relevant Files

| File | Responsibility |
|------|----------------|
| `src/upgrade.rs` | Distro detection, prerequisite checks, upgrade execution logic |
| `src/ui/upgrade_page.rs` | GTK4/libadwaita upgrade page widget |
| `src/backends/nix.rs` | Nix/NixOS backend for package *updates* (not major version upgrades) |
| `src/backends/mod.rs` | `Backend` trait definition |

### 1.2 Existing NixOS Infrastructure

The project already implements:

- **`NixOsConfigType` enum** (`src/upgrade.rs`, line ~22):
  ```rust
  pub enum NixOsConfigType {
      Flake,
      LegacyChannel,
  }
  ```
- **`detect_nixos_config_type()`** — checks for `/etc/nixos/flake.nix` to distinguish flake vs channel-based systems.
- **`detect_hostname()`** — reads `/proc/sys/kernel/hostname`.
- **`check_nixos_upgrade(version_id: &str) -> String`** — parses current YY.MM version, computes the next channel (May→November, November→next year May), and checks `https://channels.nixos.org/nixos-YY.MM` via HTTP for availability.
- **`upgrade_nixos(tx)`** — dispatches on config type:
  - `LegacyChannel`: runs `nix-channel --update && nixos-rebuild switch --upgrade` (only updates, does **not** switch to a new major channel)
  - `Flake`: runs `nix flake update /etc/nixos && nixos-rebuild switch --flake /etc/nixos#hostname` (also an update, not a version upgrade)
- **`upgrade_page.rs`** — detects `(NixOsConfigType, hostname)` on a background thread and shows a "NixOS Config Type" row in the System Information group. The "Start Upgrade" button (when confirmed) unconditionally calls `execute_upgrade()` for all distros including NixOS.

### 1.3 Problems Identified

**Problem 1 — Flake-managed NixOS systems should not be upgraded via the app.**  
On a flake-managed NixOS system, a major version upgrade (e.g., 24.05 → 24.11) requires the user to change the `nixpkgs` input URL in their `/etc/nixos/flake.nix` (e.g., from `github:NixOS/nixpkgs/nixos-24.05` to `github:NixOS/nixpkgs/nixos-24.11`) and then run `nix flake update`. The app cannot and should not do this automatically because the flake is the user's configuration file and may have other inputs, overlays, or pinned dependencies that must remain under user control. The current `upgrade_nixos()` for Flake only updates flake inputs (same channel) — it does not perform a major version upgrade at all.

**Problem 2 — LegacyChannel upgrade does not switch channels.**  
The current `upgrade_nixos()` for `LegacyChannel` runs only:
```sh
nix-channel --update && nixos-rebuild switch --upgrade
```
This updates packages within the *current* channel. A proper NixOS major-version upgrade requires:
1. `nix-channel --add https://nixos.org/channels/nixos-YY.MM nixos` — registers the new channel URL
2. `nixos-rebuild switch --upgrade` — switches to and builds with the new channel

Without step 1, `nixos-rebuild switch --upgrade` only rebuilds on the same channel.

---

## 2. Problem Definition

The NixOS upgrade tab needs two distinct behavioral paths:

1. **NixOS + Flakes** — Show the user that a next major NixOS version is available, but inform them (via a UI dialog and persistent banner) that they must upgrade by editing their `flake.nix` rather than through the app. The "Upgrade" button must NOT attempt to execute a channel switch.

2. **NixOS without Flakes (LegacyChannel)** — Perform an actual channel-based major version upgrade: switch the `nixos` channel to the next stable release URL, then run `nixos-rebuild switch --upgrade`.

---

## 3. Research Sources

The following sources were consulted:

1. **NixOS Manual — Upgrading NixOS** (`https://nixos.org/manual/nixos/stable/#sec-upgrading`):  
   Channel upgrade process: `nix-channel --add https://nixos.org/channels/nixos-XX.YY nixos`, then `nixos-rebuild switch --upgrade`. Flake users must update the `nixpkgs` input URL in `flake.nix` manually.

2. **NixOS Channels listing** (`https://channels.nixos.org/`):  
   NixOS releases follow a YY.MM versioning scheme. Stable channels: `nixos-YY.05` (May) and `nixos-YY.11` (November). Next channel URL pattern: `https://nixos.org/channels/nixos-YY.MM`.

3. **NixOS Wiki — Flakes** (`https://nixos.wiki/wiki/Flakes`):  
   Flake-based NixOS systems manage NixOS channel via a `nixpkgs` input in `flake.nix`. To upgrade to a new NixOS release, the user changes the `nixpkgs` URL (e.g., `github:NixOS/nixpkgs/nixos-24.11`) and runs `nix flake update`. This is inherently a manual/user-controlled operation.

4. **NixOS Wiki — Upgrade NixOS** (`https://nixos.wiki/wiki/NixOS_Installation_Guide#Upgrading_NixOS`):  
   Legacy (non-flake) channel upgrade using `nix-channel --add` and `nixos-rebuild switch --upgrade` is the canonical approach. The channel must be switched before `nixos-rebuild`.

5. **libadwaita API Documentation — AdwBanner** (`https://gnome.pages.gitlab.gnome.org/libadwaita/doc/1-latest/`):  
   `AdwBanner` (stable since libadwaita 1.3) is a strip banner widget placed at the top of a page to display informational, warning, or action-required messages. Supports `.set_revealed(bool)` for deferred display. The project already enables the `v1_5` feature which includes `AdwBanner`.

6. **libadwaita API Documentation — AdwAlertDialog** (`https://gnome.pages.gitlab.gnome.org/libadwaita/doc/1-latest/`):  
   `AdwAlertDialog` (stable since libadwaita 1.5) supports building dialogs with multiple response buttons and customisable response appearances. The project already uses `adw::AlertDialog` in `upgrade_page.rs` for the confirm-upgrade dialog.

7. **NixOS channels.nixos.org HTTP check** (already implemented in `check_nixos_upgrade()`):  
   A `curl -w "%{http_code}"` request to `https://channels.nixos.org/nixos-YY.MM` returns HTTP 200/301/302 when the channel exists. This pattern is already used in the project and can be reused.

8. **libadwaita-rs 0.7 crate** (`https://docs.rs/libadwaita/0.7/libadwaita/`):  
   `adw::Banner::builder()` exposes `.title()`, `.button_label()`, `.revealed()` builder properties. `banner.set_revealed(true/false)` controls visibility. The `connect_button_clicked` signal fires when the action button is clicked. Available in `libadwaita = "0.7"` with `features = ["v1_5"]`.

---

## 4. Proposed Solution Architecture

### 4.1 Overview

```
upgrade_page.rs (UpgradePage::build)
│
├── Pre-create adw::Banner (revealed=false) for "flake" advisory
│    └── Added to page_box before the scrolled window
│
├── Rc<RefCell<Option<NixOsConfigType>>> (new) — shared state
│
├── Detection callback (existing, extended):
│    └── If NixOS+Flake  → reveal banner; set nixos_config_type to Some(Flake)
│    └── If NixOS+Legacy → set nixos_config_type to Some(LegacyChannel)
│
└── Upgrade button click handler (modified):
     ├── If nixos_config_type == Some(Flake):
     │    └── Show informational AdwAlertDialog with flake upgrade instructions
     │         ("Close" button only — no destructive action)
     └── Else (LegacyChannel, or other distro):
          └── Show existing destructive confirm dialog → execute_upgrade()
               └── upgrade_nixos() now properly switches the channel URL

upgrade.rs
├── New pub fn next_nixos_channel(version_id: &str) -> Option<String>
│    (extracted from check_nixos_upgrade; returns "nixos-YY.MM" or None)
│
└── upgrade_nixos() modified:
     ├── LegacyChannel path: now receives version_id, computes next channel,
     │    runs nix-channel --add + nixos-rebuild switch --upgrade
     └── Flake path: unchanged (unreachable from upgrade tab in new flow,
          but keep for safety / direct callers if any)
```

### 4.2 NixOS Channel Computation Logic (reused)

The existing `check_nixos_upgrade()` already contains correct logic. The next channel from a given `YY.MM` version_id:

```
if month >= 11 → next = (year+1, 05)
else           → next = (year, 11)
→ channel name: "nixos-{year}.{month:02}"
→ channel URL:  "https://nixos.org/channels/nixos-{year}.{month:02}"
```

This can be extracted into a public helper `next_nixos_channel()`.

### 4.3 AdwBanner for Flake Systems

```rust
// Created once in UpgradePage::build(), starts hidden
let flake_banner = adw::Banner::builder()
    .title("Flake-managed system: upgrade via your flake.nix")
    .revealed(false)
    .build();
page_box.append(&flake_banner);  // before scrolled window
page_box.append(&scrolled);
```

When NixOS+Flake is detected, the detection callback calls `flake_banner.set_revealed(true)`.

### 4.4 Informational Dialog for Flake Users

When the "Start Upgrade" button is clicked and `nixos_config_type` is `Some(NixOsConfigType::Flake)`:

```rust
let next_channel = upgrade::next_nixos_channel(&distro.version_id)
    .unwrap_or_else(|| "nixos-YY.MM".to_string());
let next_ver = next_channel.trim_start_matches("nixos-");

let dialog = adw::AlertDialog::builder()
    .heading("Upgrade via Flake")
    .body(format!(
        "NixOS {next_ver} is available, but this system is managed with Nix Flakes.\n\n\
         To upgrade, update the nixpkgs input in your /etc/nixos/flake.nix to point \
         to the new release, then run:\n\n\
         sudo nix flake update /etc/nixos\n\
         sudo nixos-rebuild switch --flake /etc/nixos\n\n\
         Example: change\n  github:NixOS/nixpkgs/nixos-{current}\nto\n  \
         github:NixOS/nixpkgs/nixos-{next_ver}"
    ))
    .build();
dialog.add_response("close", "Close");
dialog.set_default_response(Some("close"));
dialog.set_close_response("close");
dialog.present(Some(button));
```

No destructive response is added. No upgrade thread is spawned.

### 4.5 Proper Channel Upgrade for Legacy NixOS

The `upgrade_nixos()` function for `LegacyChannel` is updated to:

```rust
NixOsConfigType::LegacyChannel => {
    // 1. Compute the next channel
    let next_channel = match next_nixos_channel(&distro.version_id) {
        Some(ch) => ch,
        None => {
            let msg = "Cannot determine next NixOS channel from version_id".to_string();
            let _ = tx.send_blocking(msg.clone());
            return Err(msg);
        }
    };
    let channel_url = format!("https://nixos.org/channels/{}", next_channel);
    
    // 2. Register the new channel
    let _ = tx.send_blocking(format!("Switching to channel {}...", next_channel));
    let add_cmd = format!(
        "{NIX_PATH_EXPORT} && nix-channel --add {} nixos",
        channel_url
    );
    if !crate::runner::run_command_sync("pkexec", &["sh", "-c", &add_cmd], tx) {
        return Err(format!("Failed to set NixOS channel to {} (see log)", next_channel));
    }

    // 3. Rebuild with the new channel
    let _ = tx.send_blocking(format!("Rebuilding NixOS on {}...", next_channel));
    if !crate::runner::run_command_sync(
        "pkexec",
        &["nixos-rebuild", "switch", "--upgrade"],
        tx,
    ) {
        return Err("Failed to rebuild NixOS with --upgrade (see log)".to_string());
    }
    Ok(())
}
```

The `upgrade_nixos()` signature changes from `(tx)` to `(distro: &DistroInfo, tx)` so it can access `version_id`.

---

## 5. UI Changes

### 5.1 New Widget: `adw::Banner` (flake advisory)

- **Widget**: `adw::Banner`
- **Location**: Prepended into `page_box` **before** the `scrolled` window (i.e., it appears above the scrolled content as a persistent top-bar strip)
- **Initial state**: `revealed = false` (hidden)
- **Title**: `"Flake-managed system: upgrade via your flake.nix"`
- **Button label**: None required (informational only; the dialog provides full details)
- **Revealed**: Set to `true` when NixOS+Flake is detected in the detection callback

### 5.2 Modified Upgrade Button Click Handler

| Scenario | Current Behavior | New Behavior |
|--------|----------------|-------------|
| NixOS + Flake | Destructive confirm dialog → runs `nix flake update + nixos-rebuild` | Informational `AdwAlertDialog` with flake.nix update instructions → no upgrade |
| NixOS + LegacyChannel | Destructive confirm dialog → `nix-channel --update + nixos-rebuild switch --upgrade` (wrong channel) | Destructive confirm dialog → `nix-channel --add <new-url> nixos + nixos-rebuild switch --upgrade` (correct) |
| Ubuntu / Fedora / etc. | No change | No change |

### 5.3 New Shared State in `UpgradePage::build()`

Add a shared `Rc<RefCell<Option<upgrade::NixOsConfigType>>>` alongside the existing `distro_info_state`:

```rust
let nixos_config_type: Rc<RefCell<Option<upgrade::NixOsConfigType>>> =
    Rc::new(RefCell::new(None));
```

This is populated in the detection callback (`nixos_extra` already carries `NixOsConfigType`), and consumed in the upgrade button click handler.

---

## 6. Implementation Steps (Detailed)

### Step 1 — Extract `next_nixos_channel()` helper in `src/upgrade.rs`

1. Add the following public function **above** `check_nixos_upgrade()`:

```rust
/// Compute the next NixOS stable channel name from a YY.MM `version_id`.
///
/// Returns `Some("nixos-YY.MM")` for the next release, or `None` if the
/// version_id cannot be parsed.
///
/// NixOS releases every six months: May (05) and November (11).
/// - If current month is ≥ 11, next is (year+1, 05)
/// - Otherwise, next is (year, 11)
pub fn next_nixos_channel(version_id: &str) -> Option<String> {
    let parts: Vec<&str> = version_id.split('.').collect();
    if parts.len() != 2 {
        return None;
    }
    let year: u32 = parts[0].parse().ok()?;
    let month: u32 = parts[1].parse().ok()?;
    let (ny, nm) = if month >= 11 { (year + 1, 5) } else { (year, 11) };
    Some(format!("nixos-{}.{:02}", ny, nm))
}
```

2. Simplify `check_nixos_upgrade()` to use `next_nixos_channel()`:

```rust
fn check_nixos_upgrade(current_version_id: &str) -> String {
    let Some(next_channel) = next_nixos_channel(current_version_id) else {
        return "Could not parse current NixOS version".to_string();
    };
    // next_channel is "nixos-YY.MM"
    let version_label = next_channel.trim_start_matches("nixos-");
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
                format!("Yes — NixOS {} is available", version_label)
            } else {
                format!("No — NixOS {} not yet available", version_label)
            }
        }
        Err(_) => "Could not check (curl not found)".to_string(),
    }
}
```

### Step 2 — Modify `upgrade_nixos()` signature in `src/upgrade.rs`

Change:
```rust
fn upgrade_nixos(tx: &async_channel::Sender<String>) -> Result<(), String> {
```
To:
```rust
fn upgrade_nixos(distro: &DistroInfo, tx: &async_channel::Sender<String>) -> Result<(), String> {
```

### Step 3 — Update `LegacyChannel` branch in `upgrade_nixos()`

Replace the existing `LegacyChannel` branch body:

```rust
NixOsConfigType::LegacyChannel => {
    let _ = tx.send_blocking("Detected: legacy channel-based NixOS config".into());

    // Determine the target channel URL
    let next_channel = match next_nixos_channel(&distro.version_id) {
        Some(ch) => ch,
        None => {
            let msg = format!(
                "Cannot determine next NixOS channel from version '{}'",
                distro.version_id
            );
            let _ = tx.send_blocking(msg.clone());
            return Err(msg);
        }
    };
    let channel_url = format!("https://nixos.org/channels/{}", next_channel);

    // Step 1: Register the new channel
    let _ = tx.send_blocking(format!(
        "Switching channel to {}...",
        next_channel
    ));
    let add_cmd = format!(
        "{NIX_PATH_EXPORT} && nix-channel --add {} nixos",
        channel_url
    );
    if !crate::runner::run_command_sync("pkexec", &["sh", "-c", &add_cmd], tx) {
        return Err(format!(
            "Failed to register NixOS channel {} (see log for details)",
            next_channel
        ));
    }

    // Step 2: Rebuild with --upgrade to apply the new channel
    let _ = tx.send_blocking(format!(
        "Rebuilding NixOS with {} (nixos-rebuild switch --upgrade)...",
        next_channel
    ));
    if !crate::runner::run_command_sync(
        "pkexec",
        &["nixos-rebuild", "switch", "--upgrade"],
        tx,
    ) {
        return Err(
            "Failed to rebuild NixOS with --upgrade (see log for details)".to_string(),
        );
    }
    Ok(())
}
```

### Step 4 — Update `execute_upgrade()` call site in `src/upgrade.rs`

In `execute_upgrade()`, change the NixOS arm:
```rust
"nixos" => upgrade_nixos(tx),
```
To:
```rust
"nixos" => upgrade_nixos(distro, tx),
```

### Step 5 — Add `nixos_config_type` shared state in `src/ui/upgrade_page.rs`

Add after the existing `distro_info_state` declaration:

```rust
let nixos_config_type: Rc<RefCell<Option<upgrade::NixOsConfigType>>> =
    Rc::new(RefCell::new(None));
```

### Step 6 — Pre-create `adw::Banner` and add to `page_box` in `src/ui/upgrade_page.rs`

In `UpgradePage::build()`, before `page_box.append(&scrolled)`:

```rust
let flake_banner = adw::Banner::builder()
    .title("Flake-managed system: upgrade via your flake.nix")
    .revealed(false)
    .build();
page_box.append(&flake_banner);
page_box.append(&scrolled);
```

Remove the existing bare `page_box.append(&scrolled)` (it will be replaced by the two appends above).

### Step 7 — Populate `nixos_config_type` in detection callback in `src/ui/upgrade_page.rs`

In the `glib::spawn_future_local` detection callback, after the `nixos_extra` branch sets up the config row, also set the shared state and reveal the banner if applicable. Add clones for new state before the closure:

```rust
let nixos_config_type_fill = nixos_config_type.clone();
let flake_banner_fill = flake_banner.clone();
```

Inside the detection callback, within the `if let Some((config_type, raw_hostname)) = &nixos_extra` block, add after creating the config_row:

```rust
// Store config type for button handler
*nixos_config_type_fill.borrow_mut() = Some(config_type.clone());

// Reveal flake banner if applicable
if *config_type == upgrade::NixOsConfigType::Flake {
    flake_banner_fill.set_revealed(true);
}
```

### Step 8 — Modify upgrade button click handler in `src/ui/upgrade_page.rs`

Before the button handler closure, clone the new state:

```rust
let nixos_config_type_for_upgrade = nixos_config_type.clone();
```

In the `upgrade_button.connect_clicked` closure, replace the existing dialog construction with a conditional:

```rust
upgrade_button.connect_clicked(move |button| {
    let distro = distro_state_for_upgrade
        .borrow()
        .clone()
        .expect("distro info must be available before upgrade button is active");

    // --- NixOS Flake: informational dialog only, no upgrade ---
    if *nixos_config_type_for_upgrade.borrow() == Some(upgrade::NixOsConfigType::Flake) {
        let next_ch = upgrade::next_nixos_channel(&distro.version_id)
            .unwrap_or_else(|| "the next NixOS release".to_string());
        let next_ver = next_ch.trim_start_matches("nixos-").to_string();
        let current_ver = distro.version_id.clone();

        let dialog = adw::AlertDialog::builder()
            .heading("Upgrade via Flake")
            .body(format!(
                "NixOS {next_ver} may be available, but this system uses Nix Flakes.\n\n\
                 To upgrade, edit /etc/nixos/flake.nix and update your nixpkgs input \
                 to point to the new release:\n\n\
                 \u{2022} Change:  github:NixOS/nixpkgs/nixos-{current_ver}\n\
                 \u{2022} To:      github:NixOS/nixpkgs/nixos-{next_ver}\n\n\
                 Then run:\n\
                 \u{2022} sudo nix flake update /etc/nixos\n\
                 \u{2022} sudo nixos-rebuild switch --flake /etc/nixos"
            ))
            .build();
        dialog.add_response("close", "Close");
        dialog.set_default_response(Some("close"));
        dialog.set_close_response("close");
        dialog.present(Some(button));
        return; // do NOT proceed to upgrade
    }

    // --- All other distros (including NixOS LegacyChannel): destructive confirm ---
    let dialog = adw::AlertDialog::builder()
        .heading("Confirm System Upgrade")
        .body(format!(
            "This will upgrade {} from version {} to the next major release.\n\n\
            This operation may take a long time and require a reboot.\n\n\
            Are you sure you want to continue?",
            distro.name, distro.version
        ))
        .build();

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("upgrade", "Upgrade");
    dialog.set_response_appearance("upgrade", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    // ... rest of existing handler (log panel, spawn thread, etc.) unchanged
});
```

The rest of the existing handler (the `connect_response` body that spawns the upgrade thread) remains unchanged — it is only reached for non-flake systems.

---

## 7. File-Level Changes Required

| File | Change Type | Description |
|------|-------------|-------------|
| `src/upgrade.rs` | Add function | `pub fn next_nixos_channel(version_id: &str) -> Option<String>` |
| `src/upgrade.rs` | Refactor | Simplify `check_nixos_upgrade()` to use `next_nixos_channel()` |
| `src/upgrade.rs` | Modify function | Change `upgrade_nixos(tx)` → `upgrade_nixos(distro: &DistroInfo, tx)` |
| `src/upgrade.rs` | Modify branch | `LegacyChannel` path: add `nix-channel --add` step before `nixos-rebuild` |
| `src/upgrade.rs` | Modify call site | `execute_upgrade()`: pass `distro` to `upgrade_nixos()` |
| `src/ui/upgrade_page.rs` | Add state | New `nixos_config_type: Rc<RefCell<Option<NixOsConfigType>>>` |
| `src/ui/upgrade_page.rs` | Add widget | Pre-create `adw::Banner` with `revealed(false)` |
| `src/ui/upgrade_page.rs` | Modify widget tree | Add banner to `page_box` before the scrolled window |
| `src/ui/upgrade_page.rs` | Modify callback | Detection callback: reveal banner + set shared state when NixOS+Flake detected |
| `src/ui/upgrade_page.rs` | Modify handler | Upgrade button: branch on `NixOsConfigType::Flake` to show informational dialog |

---

## 8. Dependencies

**No new Cargo dependencies are required.**

All required APIs are already available:

| API | Crate | Version/Feature | Status |
|-----|-------|-----------------|--------|
| `adw::Banner` | `libadwaita` | v0.7, `features = ["v1_5"]` (Banner available since libadwaita 1.3) | ✅ Already available |
| `adw::AlertDialog` | `libadwaita` | v0.7, `features = ["v1_5"]` | ✅ Already used in codebase |
| `adw::ResponseAppearance` | `libadwaita` | v0.7 | ✅ Already used in codebase |
| `async_channel` | `async-channel` | v2 | ✅ Already in dependencies |
| `glib::spawn_future_local` | `glib` | v0.20 | ✅ Already used in codebase |

---

## 9. Detailed Architecture Decisions

### 9.1 Why not auto-upgrade the flake?

Automatic editing of `/etc/nixos/flake.nix` would be:
- **Dangerous**: The flake may pin specific revisions, use overlays, reference self, or have non-nixpkgs inputs that interact with the nixpkgs version.
- **Impossible in general**: The `nixpkgs` input URL format varies (git+https, github:, tarball, etc.). A generic sed/replace approach would be fragile.
- **Against flake design principles**: Flakes are deterministic configurations managed by the user. The lockfile is the source of truth.

The correct approach is to inform and instruct.

### 9.2 Why use `adw::Banner` instead of `adw::InfoBar` or a label?

`adw::Banner` is the modern libadwaita 1.3+ component for persistent top-of-page informational strips. It:
- Is automatically styled to look appropriate in the GNOME HIG
- Has built-in hide/show via `.set_revealed()`  
- Uses smooth reveal animations
- Can include an optional action button

`GtkInfoBar` is deprecated in GTK4. A plain label would not have the visual weight to communicate the advisory clearly.

### 9.3 Why not use `adw::StatusPage`?

`adw::StatusPage` replaces the entire page content, which would prevent showing system info rows and prerequisite checks that are still useful even on flake systems. A `Banner` allows the full page to remain functional.

### 9.4 Channel URL format

The canonical channel registration URL is:
```
https://nixos.org/channels/nixos-YY.MM
```
This is the standard URL documented in the NixOS manual and used by `nix-channel --add`. The channel is named `nixos` (the channel alias used by NixOS configuration.nix). Do NOT use `nixos-small` or other variants.

### 9.5 Security considerations for channel URL

The channel URL is constructed from the `version_id` field parsed from `/etc/os-release`. The format is strictly `YY.MM` (two integers separated by a dot), and the `next_nixos_channel()` function only accepts this format and produces output of the form `nixos-YY.MM`. No user-controlled input is used. The URL is passed to `nix-channel` as a separate argument (not shell-interpolated without validation).

The `nix-channel --add <url> nixos` command is run via `pkexec sh -c "... && nix-channel --add <url> nixos"`. The URL is embedded in the shell command string. To avoid command injection, the URL must be validated to contain only characters safe for shell embedding. Since `next_nixos_channel()` produces output of format `nixos-\d{2}\.\d{2}`, the resulting URL `https://nixos.org/channels/nixos-XX.YY` contains only alphanumeric, colon, slash, hyphen, and dot characters — all safe in a double-quoted shell string. No additional validation is required beyond what `next_nixos_channel()` already enforces by construction.

---

## 10. Risks and Mitigations

| Risk | Severity | Mitigation |
|------|----------|------------|
| User is on a flake system where `flake.nix` does not follow standard `github:NixOS/nixpkgs/nixos-X.Y` format | Medium | The dialog shows the general principle (update the `nixpkgs` input) rather than hardcoding a specific format. The user is expected to understand their own flake config. |
| `next_nixos_channel()` returns `None` for unstable or custom version strings | Low | Both `check_nixos_upgrade()` and the upgrade dialog gracefully handle `None` with a fallback message. The "Start Upgrade" button is only enabled when `check_upgrade_available()` returns a "Yes —" string, which already requires successful parsing. |
| Channel upgrade (`nix-channel --add`) succeeds but `nixos-rebuild switch --upgrade` fails mid-way | Medium | The system retains its previous generation and can roll back via `nixos-rebuild switch --rollback` or boot menu. The log panel shows full output for diagnosis. |
| `nix-channel --add` registers a channel for a version that hasn't passed QA (e.g., recently released, not yet stable) | Low | The app only suggests upgrading when `channels.nixos.org` returns HTTP 200/301/302, meaning the channel is publicly available. The user still explicitly confirms the destructive upgrade dialog. |
| `adw::Banner` overlaps with content on small screens | Low | `adw::Banner` is designed for adaptive layouts. The `adw::Clamp` already constrains content width. The banner sits outside the clamp in the main `page_box`, which is the correct placement. |
| Flake path in `upgrade_nixos()` becomes unreachable from the upgrade tab | Low | The function is still reachable from tests; keep the flake branch for correctness. Add a comment noting that the flake path is not invoked from the upgrade tab (informational dialog is shown instead). |

---

## 11. Test Impact

The project has existing unit tests in `src/upgrade.rs`:
- New tests should be added for `next_nixos_channel()`:
  - `next_nixos_channel("24.05")` → `Some("nixos-24.11")`
  - `next_nixos_channel("24.11")` → `Some("nixos-25.05")`
  - `next_nixos_channel("invalid")` → `None`
  - `next_nixos_channel("")` → `None`
  - `next_nixos_channel("24")` → `None` (not two parts)

These are cheap unit tests with no I/O that fit the existing test pattern.

---

## 12. Summary of Changes

Given the analysis above, the full implementation requires changes to **exactly two files**:

1. **`src/upgrade.rs`** — Add `next_nixos_channel()` helper, refactor `check_nixos_upgrade()` to use it, update `upgrade_nixos()` to accept `distro: &DistroInfo` and perform a proper channel switch for legacy systems.

2. **`src/ui/upgrade_page.rs`** — Add `adw::Banner` widget, add `nixos_config_type` shared state, modify detection callback to populate state and reveal banner, modify upgrade button handler to branch on flake vs non-flake.

No new crate dependencies, no schema changes, no Meson/Nix/Flatpak changes required.

---

*Spec file path: `.github/docs/subagent_docs/nixos_upgrade_tab_spec.md`*
