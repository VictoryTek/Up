# Specification: Determinate Nix Backend & Distro Upgrade Detection Fixes

**Date:** 2026-04-24  
**Project:** Up — GTK4/libadwaita Linux System Updater (Rust)  
**Status:** DRAFT — Phase 1 Specification  

---

## Table of Contents

1. [Current State Analysis](#1-current-state-analysis)
2. [Feature 1: Determinate Nix Backend](#2-feature-1-determinate-nix-backend)
3. [Feature 2: Distro Upgrade Detection Fixes](#3-feature-2-distro-upgrade-detection-fixes)
4. [Implementation Steps (File-by-File)](#4-implementation-steps-file-by-file)
5. [New Dependencies](#5-new-dependencies)
6. [Risks and Mitigations](#6-risks-and-mitigations)

---

## 1. Current State Analysis

### 1.1 Backend Architecture

The backend system lives in `src/backends/`. The `Backend` trait (defined in `mod.rs`) requires:

```
kind()           → BackendKind
display_name()   → &str
description()    → &str
icon_name()      → &str
run_update()     → Pin<Box<dyn Future<Output = UpdateResult> + Send + '_>>
needs_root()     → bool          (default: false)
count_available() → Pin<Box<…>>  (default: Ok(0))
list_available()  → Pin<Box<…>>  (default: Ok(vec![]))
```

`BackendKind` is an enum with variants: `Apt`, `Dnf`, `Pacman`, `Zypper`, `Flatpak`, `Homebrew`, `Nix`.

`detect_backends()` in `mod.rs` calls each detection function and builds the active backend list:
1. `os_package_manager::detect()` — APT / DNF / Pacman / Zypper
2. `nix::is_available()` → `NixBackend`
3. `flatpak::is_available()` → `FlatpakBackend`
4. `homebrew::is_available()` → `HomebrewBackend`

### 1.2 Existing Nix Backend (`src/backends/nix.rs`)

**Detection:**
```rust
pub fn is_available() -> bool {
    which::which("nix").is_ok()
}
```
This checks only for the `nix` binary — it does NOT distinguish between upstream Nix and Determinate Nix.

**Update paths in `run_update()`:**

| Condition | Command |
|---|---|
| NixOS + flake | `pkexec env PATH=… sh -c "nix flake update … && nixos-rebuild switch --flake …#attr"` |
| NixOS + legacy channels | `pkexec env PATH=… sh -c "nix-channel --update && nixos-rebuild switch"` |
| Non-NixOS + flake profile (manifest v2) | `nix profile upgrade ".*"` |
| Non-NixOS + legacy profile | `nix-env -u` |

The flake attribute for NixOS rebuilds is read from `/etc/nixos/vexos-variant` (VexOS-specific).

**Missing:** No support for upgrading the Determinate Nix installation itself.

### 1.3 Distro Upgrade Detection (`src/upgrade.rs`)

#### `detect_distro()` — `upgrade_supported` field:
```rust
let upgrade_supported = matches!(
    id.as_str(),
    "ubuntu" | "fedora" | "opensuse-leap" | "debian" | "nixos"
);
```

The `id` value is read from the `ID=` key in `/etc/os-release`.

#### `check_upgrade_available()` dispatch:
```rust
match distro.id.as_str() {
    "ubuntu"        => check_ubuntu_upgrade(),       // do-release-upgrade -c
    "fedora"        => check_fedora_upgrade(…),      // curl HTTP check
    "debian"        => check_debian_upgrade(),        // "Check manually" — NON-FUNCTIONAL
    "opensuse-leap" => check_opensuse_upgrade(),      // "Check manually" — NON-FUNCTIONAL
    "nixos"         => check_nixos_upgrade(…),        // curl HTTP check
    _               => "Not supported…"
}
```

#### `execute_upgrade()` dispatch:
```rust
match distro.id.as_str() {
    "ubuntu" | "debian" => upgrade_ubuntu(tx),   // ← "debian" incorrectly calls upgrade_ubuntu!
    "fedora"            => upgrade_fedora(tx),
    "opensuse-leap"     => upgrade_opensuse(tx),
    "nixos"             => upgrade_nixos(distro, tx),
    _                   => Err(…)
}
```

#### The Upgrade Tab visibility (in `src/ui/window.rs`):
```rust
upgrade_stack_page.set_visible(info.upgrade_supported);
```
The tab is shown for `debian` and `opensuse-leap` even though their checks return placeholder "check manually" strings — this is misleading UX.

### 1.4 Identified Bugs and Gaps

| # | Location | Severity | Description |
|---|---|---|---|
| B1 | `upgrade.rs` `execute_upgrade()` | **CRITICAL** | `"debian"` calls `upgrade_ubuntu()` which invokes `do-release-upgrade`. This binary is Ubuntu-specific and does not exist on Debian. |
| B2 | `upgrade.rs` `check_debian_upgrade()` | **MEDIUM** | Returns a static manual-check string. The upgrade tab is shown but the check is meaningless. |
| B3 | `upgrade.rs` `check_opensuse_upgrade()` | **MEDIUM** | Returns a static manual-check string. The upgrade tab is shown but the check is meaningless. |
| B4 | `backends/nix.rs` | **MEDIUM** | No detection or support for Determinate Nix self-upgrade (`determinate-nixd upgrade`). |
| B5 | `backends/mod.rs` | **LOW** | No `DeterminateNix` backend kind or module registration. |
| B6 | `upgrade.rs` `detect_distro()` | **LOW** | `upgrade_supported` does not verify that upgrade tools (`do-release-upgrade`, etc.) are installed. A system could have `ID=ubuntu` but no `do-release-upgrade`. |

---

## 2. Feature 1: Determinate Nix Backend

### 2.1 What Is Determinate Nix?

Determinate Nix is an alternative Nix installer and distribution from [Determinate Systems](https://determinate.systems/). Key characteristics:

- Installed via: `curl -fsSL https://install.determinate.systems/nix | sh -s -- install --determinate`
- Creates `/nix/receipt.json` — a JSON file with installation metadata. **This file is the canonical detection marker.** It is created exclusively by the Determinate installer and does not exist in upstream Nix installations.
- Runs `determinate-nixd` as a system daemon. The binary is located at `/nix/var/nix/profiles/default/bin/determinate-nixd` or found via `PATH`.
- For package updates on non-NixOS systems: behaves identically to upstream Nix (`nix profile upgrade`, `nix-env -u`).
- For self-upgrade of the Nix installation: uses `sudo determinate-nixd upgrade`.

**Source:** Determinate Systems official documentation: https://docs.determinate.systems/determinate-nix

### 2.2 Problem Definition

The existing `NixBackend` handles Nix package updates correctly for both upstream and Determinate Nix (since the package update commands are identical). However, Up currently has **no capability to upgrade the Determinate Nix installation itself** (i.e., the `determinate-nixd` daemon and bundled `nix` binary).

This is a distinct update operation — analogous to updating a package manager itself, not just the packages it manages.

### 2.3 Proposed Architecture

Add a new, independent backend: `DeterminateNixBackend`.

**Separation of concerns:**
- `NixBackend` → updates packages in the Nix profile / NixOS system
- `DeterminateNixBackend` → upgrades the Determinate Nix installation itself

Both backends can coexist when Determinate Nix is installed on a non-NixOS system. Both are shown in the UI as separate update rows.

**Detection precedence:** Determinate Nix is always a non-NixOS installation because `/nix/receipt.json` is not present on NixOS (NixOS manages Nix via its own module system). The `DeterminateNixBackend` therefore never activates on NixOS.

### 2.4 Detection Logic

```rust
/// Returns true when Determinate Nix (by Determinate Systems) is installed.
///
/// Detection uses two markers in conjunction:
/// 1. `/nix/receipt.json` — created exclusively by the Determinate Nix installer.
///    This file is the canonical indicator and is NOT present in upstream Nix.
/// 2. `determinate-nixd` binary on PATH — confirms the daemon is installed and
///    the installation is complete (avoids false positives from partial installs).
pub fn is_available() -> bool {
    std::path::Path::new("/nix/receipt.json").exists()
        && which::which("determinate-nixd").is_ok()
}
```

**Why both checks?**
- `/nix/receipt.json` alone: could be a stale file from a previous installation that was partially uninstalled.
- `determinate-nixd` alone: the binary could theoretically exist outside of a Determinate Nix install (unlikely but possible in exotic configurations).
- Together: high confidence of an active Determinate Nix installation.

### 2.5 Update Command

From Determinate Systems official documentation:

```bash
sudo determinate-nixd upgrade
```

In Up's pkexec-based privilege model, this becomes:

```
pkexec determinate-nixd upgrade
```

`pkexec` resets `PATH`, so the binary path must be explicit or PATH must be restored. The binary is typically at `/nix/var/nix/profiles/default/bin/determinate-nixd`. Use the same `env PATH=…` pattern used by the existing Nix backend:

```
pkexec env PATH=/nix/var/nix/profiles/default/bin:/run/wrappers/bin sh -c "determinate-nixd upgrade"
```

### 2.6 Version / Count Detection

`determinate-nixd version` outputs the current version and indicates whether an upgrade is available. Sample output:

```
Determinate Nix v3.6.2
An upgrade is available: v3.7.0
Run `sudo determinate-nixd upgrade` to upgrade.
```

**`count_available()` strategy:**
- Run `determinate-nixd version` as unprivileged.
- If output contains "An upgrade is available" → return `Ok(1)`.
- If the command succeeds without that phrase → return `Ok(0)`.
- On error → return `Err(message)`.

**`list_available()` strategy:**
- Same as above; if upgrade available, return `vec!["determinate-nix".to_string()]`.

### 2.7 `needs_root()`

Returns `true`. Upgrading `determinate-nixd` modifies `/nix` which is a system directory requiring root.

### 2.8 UpdateResult Counting

The `run_update()` success path parses the command output:
- If the output contains "Nothing to upgrade" or "already up to date" (case-insensitive) → `updated_count: 0`
- If the output contains "Upgraded" or "upgrading" or "successfully" → `updated_count: 1`
- Default on unrecognised output → `updated_count: 1` (assume something happened since the command succeeded)

---

## 3. Feature 2: Distro Upgrade Detection Fixes

### 3.1 Problem Summary

Three categories of problems:

**Category A — Wrong behaviour (bugs):**
1. `debian` in `execute_upgrade()` calls `upgrade_ubuntu()` which runs `do-release-upgrade` — Ubuntu-specific, doesn't exist on Debian.

**Category B — Non-functional upgrade checks (broken UX):**
2. `check_debian_upgrade()` always returns a static "check manually" string. The upgrade tab is shown but the check is useless.
3. `check_opensuse_upgrade()` always returns a static "check manually" string. Same issue.

**Category C — Missing tool availability guard:**
4. `upgrade_supported` is set at distro detection time without verifying that upgrade tools are installed. This can show the Upgrade tab on Ubuntu when `do-release-upgrade` is absent.

### 3.2 Fix A: Remove Debian From upgrade_supported

Debian upgrades follow a different process than Ubuntu (apt source list editing, `apt-get dist-upgrade`, no `do-release-upgrade` by default). Implementing a full Debian dist-upgrade is out of scope for this feature and the existing code is actively wrong.

**Fix:** Remove `"debian"` from the `upgrade_supported` match and from `execute_upgrade()`.

```rust
// In detect_distro():
let upgrade_supported = matches!(
    id.as_str(),
    "ubuntu" | "fedora" | "opensuse-leap" | "nixos"
    // "debian" intentionally removed — no safe automated upgrade path
);

// In execute_upgrade():
match distro.id.as_str() {
    "ubuntu"        => upgrade_ubuntu(tx),
    "fedora"        => upgrade_fedora(tx),
    "opensuse-leap" => upgrade_opensuse(tx),
    "nixos"         => upgrade_nixos(distro, tx),
    _ => Err(format!("Upgrade not supported for '{}'.", distro.name))
}
```

Also remove `check_debian_upgrade()` since it is now unreachable.

### 3.3 Fix B: Implement openSUSE Leap Upgrade Check

openSUSE Leap provides a stable release cycle (e.g., 15.5 → 15.6 → 16.0). The next version can be determined by parsing `/etc/os-release`'s `VERSION_ID` and incrementing the minor component.

**Updated `check_opensuse_upgrade()` strategy:**
1. Parse `VERSION_ID` from `/etc/os-release` (e.g., `"15.5"`).
2. Compute next version (e.g., `"15.6"`).
3. Check availability by probing the openSUSE release mirror using `curl`:
   ```
   https://download.opensuse.org/distribution/leap/{next_version}/repo/oss/
   ```
   HTTP 200/301/302 → available; other codes → not released.
4. If `curl` is absent, fall back to: `"Could not check (curl not found)"`.

**Helper function to add:**
```rust
fn next_opensuse_leap_version(version_id: &str) -> Option<String> {
    // VERSION_ID for openSUSE Leap is "X.Y" (e.g., "15.5")
    let parts: Vec<&str> = version_id.split('.').collect();
    if parts.len() != 2 {
        return None;
    }
    let major: u32 = parts[0].parse().ok()?;
    let minor: u32 = parts[1].parse().ok()?;
    Some(format!("{}.{}", major, minor + 1))
}
```

Updated function signature:
```rust
fn check_opensuse_upgrade(version_id: &str) -> String { … }
```

The `check_upgrade_available()` call site must pass `&distro.version_id`:
```rust
"opensuse-leap" => check_opensuse_upgrade(&distro.version_id),
```

### 3.4 Fix C: Ubuntu — Upgrade Tool Availability Guard

The `upgrade_supported` flag for Ubuntu is unconditional. `do-release-upgrade` is not always installed (e.g., minimal Ubuntu server installs, containers). The Upgrade tab should only be visible when the tool is present.

**Updated `detect_distro()` for Ubuntu:**
```rust
let upgrade_supported = match id.as_str() {
    "ubuntu"        => which::which("do-release-upgrade").is_ok(),
    "fedora"        => true,   // dnf system-upgrade plugin is installed dynamically
    "opensuse-leap" => true,
    "nixos"         => true,
    _               => false,
};
```

This guards the Ubuntu Upgrade tab behind the actual tool being available.

### 3.5 `DistroInfo` Struct — Pass `version_id` to Check Functions

The `check_upgrade_available()` function receives a `&DistroInfo`, so `version_id` is already accessible. The `check_opensuse_upgrade()` function must be updated to accept `version_id` as a parameter (currently it ignores it).

Current broken signature:
```rust
fn check_opensuse_upgrade() -> String {
    "Check manually at https://get.opensuse.org/leap/".to_string()
}
```

New signature:
```rust
fn check_opensuse_upgrade(version_id: &str) -> String { … }
```

---

## 4. Implementation Steps (File-by-File)

### 4.1 `src/backends/mod.rs`

**Step 1:** Add `pub mod determinate_nix;` to the module declarations.

**Step 2:** Add `DeterminateNix` to the `BackendKind` enum:
```rust
pub enum BackendKind {
    Apt,
    Dnf,
    Pacman,
    Zypper,
    Flatpak,
    Homebrew,
    Nix,
    DeterminateNix,   // ← NEW
}
```

**Step 3:** Add the `Display` arm for `DeterminateNix`:
```rust
Self::DeterminateNix => write!(f, "Determinate Nix"),
```

**Step 4:** In `detect_backends()`, add detection after the `NixBackend` block:
```rust
// Determinate Nix — self-upgrade of the Determinate Nix installation.
// Added after NixBackend so NixBackend runs first for package updates.
if determinate_nix::is_available() {
    backends.push(Arc::new(determinate_nix::DeterminateNixBackend));
}
```

### 4.2 `src/backends/determinate_nix.rs` (NEW FILE)

Create this file with the following structure:

```rust
use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::runner::CommandRunner;
use std::future::Future;
use std::pin::Pin;

/// Returns true when Determinate Nix (by Determinate Systems) is installed.
///
/// Uses two markers in conjunction:
/// 1. `/nix/receipt.json` — created exclusively by the Determinate Nix installer.
/// 2. `determinate-nixd` binary on PATH — confirms the daemon is active.
pub fn is_available() -> bool {
    std::path::Path::new("/nix/receipt.json").exists()
        && which::which("determinate-nixd").is_ok()
}

/// Parse `determinate-nixd version` output to detect if an upgrade is available.
///
/// Returns `true` if the output contains the phrase "An upgrade is available"
/// (the canonical indicator from Determinate Systems documentation).
fn upgrade_available_in_output(output: &str) -> bool {
    output
        .lines()
        .any(|l| l.to_ascii_lowercase().contains("an upgrade is available"))
}

/// Parse upgraded/already-up-to-date from `determinate-nixd upgrade` output.
fn count_determinate_upgraded(output: &str) -> usize {
    let lower = output.to_ascii_lowercase();
    // "nothing to upgrade" or "already" indicates no change
    if lower.contains("nothing to upgrade") || lower.contains("already up to date") || lower.contains("already on the latest") {
        return 0;
    }
    // Any mention of "upgraded", "upgrading", or "successfully" = 1 component upgraded
    if lower.contains("upgraded") || lower.contains("upgrading") || lower.contains("successfully") {
        return 1;
    }
    // Default: command succeeded, assume something changed
    1
}

pub struct DeterminateNixBackend;

impl Backend for DeterminateNixBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::DeterminateNix
    }

    fn display_name(&self) -> &str {
        "Determinate Nix"
    }

    fn description(&self) -> &str {
        "Determinate Nix installation (determinate-nixd)"
    }

    fn icon_name(&self) -> &str {
        "system-software-install-symbolic"
    }

    fn needs_root(&self) -> bool {
        true
    }

    fn run_update<'a>(
        &'a self,
        runner: &'a CommandRunner,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            // pkexec resets PATH; restore the Nix binary directory explicitly.
            // `determinate-nixd upgrade` upgrades the Determinate Nix installation
            // to the latest version advised by Determinate Systems.
            match runner
                .run(
                    "pkexec",
                    &[
                        "env",
                        "PATH=/nix/var/nix/profiles/default/bin:/run/wrappers/bin",
                        "sh",
                        "-c",
                        "determinate-nixd upgrade",
                    ],
                )
                .await
            {
                Ok(output) => UpdateResult::Success {
                    updated_count: count_determinate_upgraded(&output),
                },
                Err(e) => UpdateResult::Error(e),
            }
        })
    }

    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async move {
            // `determinate-nixd version` is unprivileged and reports whether
            // an upgrade is available.
            let out = tokio::process::Command::new("determinate-nixd")
                .arg("version")
                .output()
                .await
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let combined = format!("{text}\n{stderr}");
            if upgrade_available_in_output(&combined) {
                Ok(1)
            } else {
                Ok(0)
            }
        })
    }

    fn list_available(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
        Box::pin(async move {
            let out = tokio::process::Command::new("determinate-nixd")
                .arg("version")
                .output()
                .await
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let combined = format!("{text}\n{stderr}");
            if upgrade_available_in_output(&combined) {
                Ok(vec!["determinate-nix".to_string()])
            } else {
                Ok(Vec::new())
            }
        })
    }
}
```

### 4.3 `src/upgrade.rs`

**Step 1: Remove `"debian"` from `upgrade_supported` in `detect_distro()`:**

Current:
```rust
let upgrade_supported = matches!(
    id.as_str(),
    "ubuntu" | "fedora" | "opensuse-leap" | "debian" | "nixos"
);
```

Replace with:
```rust
let upgrade_supported = match id.as_str() {
    "ubuntu"        => which::which("do-release-upgrade").is_ok(),
    "fedora"        => true,
    "opensuse-leap" => true,
    "nixos"         => true,
    _               => false,
};
```

**Step 2: Fix `execute_upgrade()` — remove `"debian"` arm:**

Current:
```rust
match distro.id.as_str() {
    "ubuntu" | "debian" => upgrade_ubuntu(tx),
    …
}
```

Replace with:
```rust
match distro.id.as_str() {
    "ubuntu"        => upgrade_ubuntu(tx),
    "fedora"        => upgrade_fedora(tx),
    "opensuse-leap" => upgrade_opensuse(tx),
    "nixos"         => upgrade_nixos(distro, tx),
    _ => {
        let msg = format!(
            "Upgrade is not yet supported for '{}'. Supported: Ubuntu, Fedora, openSUSE Leap, NixOS.",
            distro.name
        );
        let _ = tx.send_blocking(msg.clone());
        Err(msg)
    }
}
```

**Step 3: Update `check_upgrade_available()` — pass `version_id` to openSUSE check:**

Current:
```rust
"opensuse-leap" => check_opensuse_upgrade(),
```

Replace with:
```rust
"opensuse-leap" => check_opensuse_upgrade(&distro.version_id),
```

Also remove the `"debian"` arm from `check_upgrade_available()`:

Current:
```rust
"debian"        => check_debian_upgrade(),
```

Remove this arm entirely.

**Step 4: Replace `check_opensuse_upgrade()` with a real implementation:**

Remove the old stub:
```rust
fn check_opensuse_upgrade() -> String {
    "Check manually at https://get.opensuse.org/leap/".to_string()
}
```

Add a helper and a real implementation:
```rust
/// Compute the next openSUSE Leap version from a "X.Y" `version_id`.
///
/// openSUSE Leap increments the minor component: 15.5 → 15.6.
/// When the minor reaches a major boundary, the major increments: 15.6 → 16.0.
/// Since we cannot predict major bumps with certainty, this function only
/// increments the minor component and lets the curl check confirm availability.
fn next_opensuse_leap_version(version_id: &str) -> Option<String> {
    let parts: Vec<&str> = version_id.split('.').collect();
    if parts.len() != 2 {
        return None;
    }
    let major: u32 = parts[0].parse().ok()?;
    let minor: u32 = parts[1].parse().ok()?;
    Some(format!("{}.{}", major, minor + 1))
}

fn check_opensuse_upgrade(version_id: &str) -> String {
    let Some(next_version) = next_opensuse_leap_version(version_id) else {
        return "Could not parse current openSUSE Leap version".to_string();
    };
    match Command::new("curl")
        .args([
            "-s",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            &format!(
                "https://download.opensuse.org/distribution/leap/{}/repo/oss/",
                next_version
            ),
        ])
        .output()
    {
        Ok(output) => {
            let code = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if code == "200" || code == "301" || code == "302" {
                format!("Yes — openSUSE Leap {} is available", next_version)
            } else {
                format!("No — openSUSE Leap {} not yet released", next_version)
            }
        }
        Err(_) => "Could not check (curl not found)".to_string(),
    }
}
```

**Step 5: Remove `check_debian_upgrade()` function** (now unreachable — delete entirely).

**Step 6: Update `check_packages_up_to_date()` — remove `"debian"` arm:**

Current:
```rust
"ubuntu" | "debian" => ("apt", &["list", "--upgradable"]),
```

Replace with:
```rust
"ubuntu" => ("apt", &["list", "--upgradable"]),
```

### 4.4 No Changes Required In Other Files

- `src/ui/window.rs` — the `upgrade_stack_page.set_visible(info.upgrade_supported)` call works correctly; the gate is now enforced in `detect_distro()`.
- `src/ui/upgrade_page.rs` — no changes needed.
- `src/app.rs`, `src/main.rs`, `src/runner.rs` — no changes needed.
- `Cargo.toml` — no new dependencies needed (see §5).

---

## 5. New Dependencies

**No new Cargo dependencies are required.**

All required capabilities are already present:
- `which` crate (v7): used for binary detection — already in `Cargo.toml`
- `tokio` with `process` feature: for async command execution — already in `Cargo.toml`
- `std::path::Path`: for `/nix/receipt.json` existence check — in stdlib
- `std::process::Command`: for `check_opensuse_upgrade()` — already used in `upgrade.rs`

---

## 6. Risks and Mitigations

### 6.1 Determinate Nix Backend

| Risk | Severity | Mitigation |
|---|---|---|
| `determinate-nixd version` output format may change between versions | LOW | The check uses `contains("an upgrade is available")` which is a documented user-facing phrase; less likely to change than structured fields. If it changes, `count_available()` falls back to `Ok(0)` (safe: no false updates). |
| `determinate-nixd upgrade` requires network access and adequate disk space | LOW | Errors surface naturally through `UpdateResult::Error(e)` in the UI log panel. No special handling needed. |
| Both `NixBackend` and `DeterminateNixBackend` running on the same system could confuse users | LOW | The display names distinguish them: "Nix" (packages) vs "Determinate Nix" (installation). The description field clarifies the purpose. |
| `/nix/receipt.json` left by a partial uninstall | LOW | The conjunction with `which::which("determinate-nixd").is_ok()` prevents false positives from stale receipt files. |
| PATH not containing `determinate-nixd` after pkexec resets it | MEDIUM | Mitigated by explicit `env PATH=/nix/var/nix/profiles/default/bin:…` in the pkexec invocation. This is the same pattern already used by the Nix backend. |

### 6.2 Distro Upgrade Detection

| Risk | Severity | Mitigation |
|---|---|---|
| openSUSE Leap version numbering may not always follow X.Y → X.(Y+1) | MEDIUM | The curl availability check is the authoritative gate; if the computed next version doesn't exist at the mirror URL, the check returns "not yet released" — this is correct behaviour. |
| `do-release-upgrade` availability check makes Ubuntu upgrade tab disappear on minimal installs | LOW | This is the desired behaviour: the tab should not show if the tool isn't present. Users who want to upgrade can install `ubuntu-release-upgrader-core` first. |
| Removing `"debian"` from `upgrade_supported` breaks Debian upgrade flow | NONE | Debian's upgrade flow was already broken (calling Ubuntu-specific `do-release-upgrade`). Removing it prevents a confusing and non-functional experience. |
| openSUSE release URL format change at download.opensuse.org | LOW | The same curl-based pattern is already used for Fedora and NixOS checks. A URL change would be a regression for all three, not just openSUSE. |
| The `upgrade_supported` logic for `"fedora"` does not check for `dnf` or `dnf-plugin-system-upgrade` availability | LOW | The `upgrade_fedora()` function already installs the plugin as Step 1 (`dnf install -y dnf-plugin-system-upgrade`). No guard needed at detection time. |

### 6.3 Test Coverage

The existing `#[cfg(test)]` module in `upgrade.rs` tests `next_nixos_channel`, `parse_df_avail_bytes`, `validate_hostname`, and `execute_upgrade` for unsupported distros.

New tests to add in `upgrade.rs`:
- `next_opensuse_leap_version("15.5")` → `Some("15.6")`
- `next_opensuse_leap_version("15.6")` → `Some("15.7")`
- `next_opensuse_leap_version("invalid")` → `None`
- `next_opensuse_leap_version("")` → `None`

New tests to add in `determinate_nix.rs`:
- `upgrade_available_in_output("An upgrade is available: v3.7.0\n…")` → `true`
- `upgrade_available_in_output("Determinate Nix v3.6.2\n")` → `false`
- `count_determinate_upgraded("Nothing to upgrade")` → `0`
- `count_determinate_upgraded("Successfully upgraded to v3.7.0")` → `1`

---

## 7. Summary of Files Changed

| File | Change Type | Description |
|---|---|---|
| `src/backends/mod.rs` | Modified | Add `pub mod determinate_nix`, `BackendKind::DeterminateNix`, Display arm, detection call in `detect_backends()` |
| `src/backends/determinate_nix.rs` | **New** | Full `DeterminateNixBackend` implementation |
| `src/upgrade.rs` | Modified | Remove `debian` from upgrade support; fix `execute_upgrade` debian arm; implement `check_opensuse_upgrade`; add `next_opensuse_leap_version`; guard Ubuntu by tool availability; remove `check_debian_upgrade` |

**Total new files:** 1  
**Total modified files:** 2

---

*Spec file path: `.github/docs/subagent_docs/determinate_nix_and_upgrade_detection_spec.md`*
