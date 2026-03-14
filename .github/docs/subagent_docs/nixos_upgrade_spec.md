# NixOS Distro Upgrade Support — Specification

**Feature:** NixOS upgrade mode  
**Spec Author:** Research & Specification Agent  
**Date:** 2026-03-14  
**Status:** Ready for Implementation

---

## 1. Current State Analysis

### 1.1 How upgrade detection and execution work today

`src/upgrade.rs` is the sole module responsible for:

| Function | Responsibility |
|---|---|
| `detect_distro()` | Parses `/etc/os-release`, returns `DistroInfo` |
| `run_prerequisite_checks()` | Runs per-distro checks; hardcoded for Ubuntu/Debian/Fedora/openSUSE |
| `execute_upgrade()` | Dispatches to per-distro upgrade functions |
| `run_streaming_command()` | Spawns a subprocess, streams stdout/stderr line-by-line to an `async_channel::Sender<String>` |

`src/ui/upgrade_page.rs` builds the GTK4/libadwaita upgrade UI. It:
- Calls `detect_distro()` at page construction time to populate System Information rows
- Runs checks via `std::thread::spawn` → `run_prerequisite_checks`
- Runs the upgrade via `std::thread::spawn` → `execute_upgrade`

### 1.2 Existing NixOS package-update backend

`src/backends/nix.rs` implements the `NixBackend` which handles **Nix profile package updates** (not OS upgrades). It uses `nix profile upgrade '.*'` (flake mode) or `nix-env -u` (legacy mode). This is entirely separate from the upgrade module — the package backend is NOT involved in what this specification describes.

### 1.3 Gap

`detect_distro()` does not include `"nixos"` in `upgrade_supported`:
```rust
let upgrade_supported = matches!(id.as_str(), "ubuntu" | "fedora" | "opensuse-leap" | "debian");
```
NixOS systems currently display "Upgrade not supported for this distribution yet" in the UI and `execute_upgrade()` prints an unsupported message and returns.

---

## 2. Research Summary

### Source 1 — NixOS Manual, §Upgrading NixOS (official)
URL: `https://nixos.org/manual/nixos/stable/index.html#sec-upgrading`

Key findings:
- NixOS uses **channels** (stable: `nixos-YY.MM`, unstable: `nixos-unstable`) as the primary update mechanism.
- The canonical upgrade command is `nixos-rebuild switch --upgrade`, which is equivalent to running `nix-channel --update nixos` followed by `nixos-rebuild switch`.
- To upgrade to a **new major release** (e.g., 24.11 → 25.05), the user must first switch the channel: `nix-channel --add https://channels.nixos.org/nixos-25.05 nixos`, then rebuild.
- All of these commands must be run as root.
- The NixOS manual shows `#` prefix for root commands; `sudo` is the standard escalation method documented in the manual.

### Source 2 — NixOS Manual, §Changing the Configuration
URL: `https://nixos.org/manual/nixos/stable/index.html#sec-changing-config`

Key findings:
- `nixos-rebuild switch` builds a new system configuration and makes it the boot default.
- Supports `--upgrade` flag to combine channel update with rebuild.
- `nixos-rebuild` is located at `/run/current-system/sw/bin/nixos-rebuild` on a live NixOS system; also typically symlinked into PATH.
- The command can produce real-time output suitable for streaming.

### Source 3 — NixOS Flakes documentation (zero-to-nix.com, nix.dev)
Key findings:
- A **flake-based** NixOS configuration is identified by the presence of `/etc/nixos/flake.nix`.
- To upgrade a flake-based system, run `nix flake update /etc/nixos` (updates all flake inputs, including `nixpkgs`), then `nixos-rebuild switch --flake /etc/nixos`.
- The `nix flake update` command writes back to `flake.lock`; because `/etc/nixos` is typically root-owned, this usually requires root access.
- A **channel-based** (legacy) system has `/etc/nixos/configuration.nix` but no `/etc/nixos/flake.nix`.

### Source 4 — `/etc/os-release` format for NixOS
Actual NixOS `os-release` content (verified from NixOS source and community docs):
```
NAME=NixOS
ID=nixos
VERSION_ID="24.11.717.3f12e35de59"
VERSION="24.11.717.3f12e35de59 (Vicuna)"
VERSION_CODENAME=vicuna
PRETTY_NAME="NixOS 24.11.717.3f12e35de59 (Vicuna)"
LOGO=nix-snowflake
HOME_URL=https://nixos.org/
DOCUMENTATION_URL=https://nixos.org/learn.html
SUPPORT_URL=https://nixos.org/community.html
BUG_REPORT_URL=https://github.com/NixOS/nixpkgs/issues
```
Key points:
- `ID=nixos` (lowercase, no quotes) — the match key.
- `VERSION_ID` includes the full build string (e.g., `24.11.717.3f12e35de59`); the human-readable part is the first two components (`24.11`).
- `VERSION` includes the codename in parentheses.
- `VERSION` and `VERSION_ID` can be used directly as display strings; the version is NOT always just a two-component release number.

### Source 5 — polkit / pkexec on NixOS
From NixOS manual and community knowledge:
- NixOS ships with polkit enabled by default (`security.polkit.enable = true`).
- `pkexec` is available and able to escalate to root if the user is in the `wheel` group.
- However, `pkexec` uses filesystem PATH lookups limited to `/usr/bin` by default on some distributions; on NixOS it does respect the PATH env if invoked correctly, but using full paths is safer.
- **Critical risk**: Unlike Ubuntu (which has defined polkit actions for `do-release-upgrade`), NixOS has no pre-shipped polkit action for `nixos-rebuild`. Using `pkexec nixos-rebuild` without explicit polkit rules may result in a "Not authorized" error.
- **Recommended approach**: Use `sudo` (not `pkexec`) for NixOS privilege escalation, consistent with the NixOS manual's own recommendations. The task specification explicitly uses `sudo nix-channel --update`, confirming `sudo` is appropriate for NixOS.
- The `nix flake update` command for the flake path can be run with `sudo` to ensure `/etc/nixos` write access.

### Source 6 — NixOS upgrade semantics vs. traditional distros
NixOS upgrade is fundamentally different from Ubuntu/Fedora:
- There is no "download-and-reboot-into-upgrade-mode" step.
- `nixos-rebuild switch` atomically builds the new system, writes the bootloader entry, and (optionally) activates it in the running system.
- Failed upgrades are recoverable by selecting an older generation at boot (GRUB menu).
- This means the upgrade is effectively **in-place and reversible**, making it safer to run from a GUI without a mandatory reboot step.
- However, some kernel or boot-related changes **do require a reboot** to take effect; `nixos-rebuild switch` will note this in its output but will not force a reboot.

---

## 3. Problem Definition

The Up application supports distro-level upgrades for Ubuntu, Fedora, openSUSE, and Debian. NixOS is a major Linux distribution with a growing user base, and Up's existing Nix package backend already demonstrates awareness of NixOS. However, the upgrade module does not:

1. Detect NixOS as a supported upgrade target.
2. Run NixOS-appropriate prerequisite checks.
3. Detect whether the system uses legacy channels or flakes.
4. Execute the correct upgrade procedure for either config type.
5. Display NixOS-specific system info in the UI (config type).

---

## 4. Proposed Solution Architecture

### 4.1 New public type: `NixOsConfigType`

Added to `src/upgrade.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NixOsConfigType {
    Flake,
    LegacyChannel,
}
```

### 4.2 New public function: `detect_nixos_config_type()`

```rust
pub fn detect_nixos_config_type() -> NixOsConfigType {
    if std::path::Path::new("/etc/nixos/flake.nix").exists() {
        NixOsConfigType::Flake
    } else {
        NixOsConfigType::LegacyChannel
    }
}
```

Detection rule:
- `/etc/nixos/flake.nix` present → `Flake`
- Otherwise → `LegacyChannel`

This is the criterion documented by the NixOS project: a flake-based config always has `flake.nix` at the root of the NixOS config directory.

### 4.3 Changes to `detect_distro()`

Add `"nixos"` to the `upgrade_supported` match:

```rust
let upgrade_supported = matches!(
    id.as_str(),
    "ubuntu" | "fedora" | "opensuse-leap" | "debian" | "nixos"
);
```

No other changes to `DistroInfo` are needed. The existing `version` (from `VERSION`) and `version_id` (from `VERSION_ID`) fields will be populated correctly from NixOS's `os-release`.

### 4.4 Changes to `run_prerequisite_checks()`

The existing `check_packages_up_to_date()` function is inapplicable to NixOS (it tries to run `apt`/`dnf`/`zypper` and panics on unrecognized distros). For NixOS:

- **Replace Check 1** ("All packages up to date") with **"nixos-rebuild available"** (`which nixos-rebuild`).
- **Keep Check 2** ("Sufficient disk space") — NixOS stores generations in `/nix/store`, which makes disk space especially important (10 GB+ remains appropriate).
- **Keep Check 3** ("Backup recommended") — still advisory/always passes.

Implementation in `run_prerequisite_checks()`:

```rust
// Check 1: change for NixOS
let packages_ok = if distro.id == "nixos" {
    check_nixos_rebuild_available()
} else {
    check_packages_up_to_date(distro)
};
```

New helper:

```rust
fn check_nixos_rebuild_available() -> CheckResult {
    if which::which("nixos-rebuild").is_ok() {
        CheckResult {
            name: "nixos-rebuild available".into(),
            passed: true,
            message: "nixos-rebuild is installed and accessible".into(),
        }
    } else {
        CheckResult {
            name: "nixos-rebuild available".into(),
            passed: false,
            message: "nixos-rebuild not found; ensure NixOS system tools are installed".into(),
        }
    }
}
```

### 4.5 Changes to `execute_upgrade()`

Add NixOS arm:

```rust
"nixos" => upgrade_nixos(tx),
```

### 4.6 New function: `upgrade_nixos()`

```rust
fn upgrade_nixos(tx: &async_channel::Sender<String>) {
    let config_type = detect_nixos_config_type();

    match config_type {
        NixOsConfigType::LegacyChannel => {
            let _ = tx.send_blocking(
                "Detected legacy channel-based NixOS configuration.".into(),
            );
            let _ = tx.send_blocking("Step 1: Updating NixOS channel...".into());
            run_streaming_command("sudo", &["nix-channel", "--update"], tx);

            let _ = tx.send_blocking(
                "Step 2: Rebuilding NixOS with upgraded packages...".into(),
            );
            run_streaming_command(
                "pkexec",
                &["nixos-rebuild", "switch", "--upgrade"],
                tx,
            );
        }
        NixOsConfigType::Flake => {
            let _ = tx.send_blocking(
                "Detected flake-based NixOS configuration.".into(),
            );
            let _ = tx.send_blocking("Step 1: Updating flake inputs in /etc/nixos...".into());
            run_streaming_command(
                "sudo",
                &["nix", "flake", "update", "/etc/nixos"],
                tx,
            );

            let _ = tx.send_blocking(
                "Step 2: Rebuilding NixOS from updated flake...".into(),
            );
            run_streaming_command(
                "pkexec",
                &["nixos-rebuild", "switch", "--flake", "/etc/nixos"],
                tx,
            );
        }
    }
}
```

**Note on `sudo` vs `pkexec`:** The task requirements specify using `sudo nix-channel --update` and `nix flake update` with `pkexec nixos-rebuild switch`. This mixed approach is adopted above. `nix-channel --update` and `nix flake update` are run with `sudo`, while `nixos-rebuild switch` uses `pkexec` consistent with other distros.

**Risk with `pkexec nixos-rebuild`:** `nixos-rebuild` is at `/run/current-system/sw/bin/nixos-rebuild` on NixOS; `pkexec` on NixOS typically resolves PATH correctly. If pkexec cannot find `nixos-rebuild`, the streaming command will emit a "Failed to start" error in the log. As a mitigation, the implementation agent should test whether `/run/current-system/sw/bin/nixos-rebuild` path needs to be used explicitly.

### 4.7 Changes to `src/ui/upgrade_page.rs`

#### 4.7.1 Version display

NixOS `VERSION_ID` may include a build hash (e.g., `24.11.717.3f12e35de59`). The `version_id` field will be populated by `detect_distro()` as-is. For display, the UI subtitle will show the full `version` string (e.g., `24.11.717.3f12e35de59 (Vicuna)`) which is informative.

No changes to the version row are required — the existing `version_row` showing `distro_info.version` will show the NixOS version correctly.

#### 4.7.2 Config type row (NixOS-specific)

Add a fourth row to the System Information group for NixOS systems showing the config type. This row is built conditionally:

```rust
// After version_row is added:
if distro_info.id == "nixos" {
    let config_type = upgrade::detect_nixos_config_type();
    let config_type_str = match config_type {
        upgrade::NixOsConfigType::Flake => "Flake-based (modern)",
        upgrade::NixOsConfigType::LegacyChannel => "Channel-based (legacy)",
    };
    let config_row = adw::ActionRow::builder()
        .title("Config Type")
        .subtitle(config_type_str)
        .build();
    config_row.add_prefix(&gtk::Image::from_icon_name("emblem-system-symbolic"));
    info_group.add(&config_row);
}
```

#### 4.7.3 Prerequisite check row labels

The first check row ("All packages up to date") has its label hardcoded in the `checks` vec in `upgrade_page.rs`. For NixOS, this should display "nixos-rebuild available". The existing code hardcodes check row labels and maps them by index to results:

```rust
let checks = vec![
    ("All packages up to date", "system-software-update-symbolic"),
    ("Sufficient disk space (10 GB+)", "drive-harddisk-symbolic"),
    ("Backup recommended", "document-save-symbolic"),
];
```

For NixOS, change the first item's label conditionally:

```rust
let first_check_label = if distro_info.id == "nixos" {
    "nixos-rebuild available"
} else {
    "All packages up to date"
};
let checks = vec![
    (first_check_label, "system-software-update-symbolic"),
    ("Sufficient disk space (10 GB+)", "drive-harddisk-symbolic"),
    ("Backup recommended", "document-save-symbolic"),
];
```

---

## 5. Files to Be Modified

| File | Change Summary |
|---|---|
| `src/upgrade.rs` | Add `NixOsConfigType` enum; `detect_nixos_config_type()`; add `"nixos"` to `upgrade_supported`; add NixOS arm in `run_prerequisite_checks()` + `execute_upgrade()`; implement `check_nixos_rebuild_available()`, `upgrade_nixos()` |
| `src/ui/upgrade_page.rs` | Conditional config-type row in system info group; conditional first check label for NixOS |

No new files are required. No new Cargo dependencies are required — `which` is already a dependency and is already used in `src/backends/nix.rs`.

---

## 6. Detailed Implementation Steps

### Step 1 — `src/upgrade.rs`

1. After the `use` statements, add the `NixOsConfigType` enum (with `#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]`).
2. Add `pub fn detect_nixos_config_type() -> NixOsConfigType { ... }` as a standalone public function.
3. In `detect_distro()`, change the `upgrade_supported` line to include `"nixos"`.
4. In `run_prerequisite_checks()`, change Check 1 to call `check_nixos_rebuild_available()` when `distro.id == "nixos"`.
5. Add `fn check_nixos_rebuild_available() -> CheckResult { ... }`.
6. In `execute_upgrade()`, add `"nixos" => upgrade_nixos(tx),` to the match arm.
7. Add `fn upgrade_nixos(tx: &async_channel::Sender<String>) { ... }`.

### Step 2 — `src/ui/upgrade_page.rs`

1. After `version_row` is added to `info_group`, add the conditional config type block for NixOS.
2. Change the hardcoded `"All packages up to date"` label to use the conditional label.

---

## 7. Risks and Edge Cases

### 7.1 `pkexec` and `nixos-rebuild` path resolution
- **Risk**: `pkexec` may fail to find `nixos-rebuild` if `/run/current-system/sw/bin` is not in the filtered PATH that pkexec uses.
- **Mitigation**: Implementation agent should test with `pkexec nixos-rebuild` and, if it fails, fall back to using the full path `pkexec /run/current-system/sw/bin/nixos-rebuild`. The `upgrade_nixos()` function should be updated if the full path is required.

### 7.2 `sudo` requires interactive authentication
- **Risk**: `sudo nix-channel --update` and `sudo nix flake update /etc/nixos` will fail if sudo requires a password and there is no TTY (the Up app is a GUI, not a terminal).
- **Mitigation**: NixOS systems with wheel group access often have `NOPASSWD` entries or `sudo` cached credentials. Alternatively, both `nix-channel --update` and `nix flake update` could be run via `pkexec` as well. The implementation agent should evaluate whether `pkexec` should be used for all four commands.
- **Alternative**: Set the first two commands to also use `pkexec` for consistency: `pkexec nix-channel --update` / `pkexec nix flake update /etc/nixos`. This is architecturally cleaner for a GUI app.

### 7.3 Flake input names
- **Risk**: `nix flake update /etc/nixos` updates **all** flake inputs. Some NixOS configurations may have other inputs (e.g., `home-manager`, `nixos-hardware`) that the user may not want to update simultaneously.
- **Mitigation**: The spec uses `nix flake update /etc/nixos` which is the standard approach. Document in UI log: "Updating all flake inputs in /etc/nixos". Advanced per-input control is out of scope.

### 7.4 NixOS "upgrade" semantics
- **Risk**: On NixOS, `nixos-rebuild switch --upgrade` updates packages within the **current channel** but does NOT switch to a new major release (e.g., 24.11 → 25.05). Major release upgrade requires the user to change the channel URL first.
- **Mitigation**: The UI dialog for confirming the upgrade should clarify: for NixOS, the upgrade updates packages to the latest in the current channel (or latest flake inputs). This is different from "next major version" semantics of Ubuntu/Fedora. The upgrade dialog body text should be adjusted for NixOS.
- **In `upgrade_page.rs`**: The `adw::AlertDialog` body is constructed from `distro_clone2.name` and `distro_clone2.version`. For NixOS this will correctly say "NixOS 24.11...". The body text "next major release" should ideally be replaced with "latest packages in the current channel/flake" for NixOS. This is a UX improvement noted for implementation.

### 7.5 Nix store disk space
- **Risk**: NixOS upgrades add new generations to the Nix store without immediately removing old ones. A system with limited disk space may run out mid-upgrade. 10 GB is a reasonable minimum but users should be warned that old generations can be cleaned up with `nix-collect-garbage`.
- **Mitigation**: The backup reminder check can optionally include a note about garbage collection, e.g., "Consider running `nix-collect-garbage -d` to free space if needed". This is advisory only.

### 7.6 `VERSION_ID` format
- **Risk**: NixOS `VERSION_ID` includes a commit hash (e.g., `24.11.717.3f12e35de59`), which the version_id field stores. The "Current Version" row in the UI will show the full hash string, which may appear verbose.
- **Mitigation**: The `version` field (from `VERSION`) is `24.11.717.3f12e35de59 (Vicuna)` which is already a clean display format and is what the UI uses. No special parsing needed.

### 7.7 Non-standard `/etc/nixos` location
- **Risk**: Some NixOS systems may store their configuration in a non-standard location (e.g., using flakes from `~/nixos` or similar).
- **Mitigation**: `/etc/nixos` is the default and documented location. The spec uses this default. Users with custom config locations are out of scope for the initial implementation.

### 7.8 Nix experimental features required for flake commands
- **Risk**: `nix flake update` requires `experimental-features = flakes nix-command` in `/etc/nix/nix.conf`. Systems using flakes typically have this enabled; if not, the command fails.
- **Mitigation**: If `/etc/nixos/flake.nix` exists but flake experimental features are not enabled, `nix flake update` will fail with an error message that streams to the UI log. This is an edge case — any system with a `flake.nix` config already has flakes enabled.

---

## 8. Expected Behaviour After Implementation

### Channel-based NixOS upgrade flow:
1. User opens Upgrade page → sees "NixOS" distribution, version (e.g., `24.11.717.3f12e35de59 (Vicuna)`), config type "Channel-based (legacy)".
2. Clicks "Run Checks" → Check rows show: "nixos-rebuild available: ✓", "Sufficient disk space: ✓/✗", "Backup recommended: ✓".
3. If all pass and backup is confirmed, "Start Upgrade" becomes active.
4. Confirmation dialog shows "NixOS" in heading.
5. On confirm: log shows:
   - `"Detected legacy channel-based NixOS configuration."`
   - `"Step 1: Updating NixOS channel..."` → streams `sudo nix-channel --update` output
   - `"Step 2: Rebuilding NixOS with upgraded packages..."` → streams `pkexec nixos-rebuild switch --upgrade` output
   - `"Command completed successfully."` or error

### Flake-based NixOS upgrade flow:
1. User opens Upgrade page → config type shows "Flake-based (modern)".
2. Same check flow.
3. On confirm: log shows:
   - `"Detected flake-based NixOS configuration."`
   - `"Step 1: Updating flake inputs in /etc/nixos..."` → streams `sudo nix flake update /etc/nixos` output
   - `"Step 2: Rebuilding NixOS from updated flake..."` → streams `pkexec nixos-rebuild switch --flake /etc/nixos` output
   - `"Command completed successfully."` or error

---

## 9. Dependencies

No new Cargo dependencies are required. The implementation uses:
- `std::path::Path::new(...).exists()` — stdlib
- `which::which("nixos-rebuild")` — `which` crate (already in `Cargo.toml` at version 7)
- `std::process::Command` (already used in `run_streaming_command`)
- `serde::{Serialize, Deserialize}` (already in scope)

---

## 10. Sources Referenced

1. **NixOS Manual § Upgrading NixOS** — `https://nixos.org/manual/nixos/stable/index.html#sec-upgrading`  
   Channel upgrade commands, channel URL format, `nixos-rebuild switch --upgrade`

2. **NixOS Manual § Changing the Configuration** — `https://nixos.org/manual/nixos/stable/index.html#sec-changing-config`  
   `nixos-rebuild switch` command, sudo requirement, rolling back via GRUB

3. **Zero to Nix — Nix Flakes Concepts** — `https://zero-to-nix.com/concepts/flakes`  
   Flake inputs, `flake.nix`, `flake.lock`, `nix flake update` semantics

4. **nix.dev — NixOS Virtual Machines Tutorial** — `https://nix.dev/tutorials/nixos/nixos-configuration-on-vm.html`  
   NixOS configuration structure, `system.stateVersion`, flake vs channel patterns

5. **NixOS source (nixpkgs)** — NixOS `os-release` template at `nixos/modules/config/os-release.nix`  
   Confirms `ID=nixos`, `VERSION_ID`, `VERSION` field format with build hash and codename

6. **Freedesktop.org OS-release specification** — Standard for `/etc/os-release` fields  
   `ID`, `VERSION_ID`, `VERSION`, `NAME`, `PRETTY_NAME` semantics and quoting rules

---

## 11. Out of Scope

- Switching to a new NixOS major release channel (e.g., 24.11 → 25.05) — this requires user action to change the channel URL before running an upgrade. Implementing channel URL management is a separate feature.
- Per-input flake updates (updating only `nixpkgs` while keeping `home-manager` pinned).
- `sudo` password prompting UI (the app relies on polkit/pkexec or existing sudo credentials).
- NixOS container or VM upgrade paths.
- Rolling back to previous NixOS generations.
