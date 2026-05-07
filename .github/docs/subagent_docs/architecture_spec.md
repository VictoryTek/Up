# Architecture & Code Quality — Implementation Specification

**Project:** Up (GTK4/libadwaita Linux desktop updater, Rust Edition 2021)  
**Scope:** Section 4 items from CODEBASE_ANALYSIS.md  
**Date:** 2026-05-06  
**Status:** SPECIFICATION — ready for implementation

---

## Decision Summary

| Item | Severity | Decision | Rationale |
|------|----------|----------|-----------|
| 4.12 Remove sort in window.rs | LOW | **GO** | 2-line delete; detection order already correct |
| 4.11b Delete dead `validate_hostname` | LOW | **GO** | Dead code tagged `#[allow(dead_code)]`; already unused |
| 4.1 `count_available` trait default | MEDIUM | **GO** | All backends run the same command for count and list |
| 4.5 `recompute_state()` closure | MEDIUM | **GO** | Self-contained; 3 sites → 1 closure in upgrade_page.rs |
| 4.4 UpdateOrchestrator extraction | MEDIUM | **PARTIAL** | Execution engine moves; GTK widget-update loop stays in window.rs |
| 4.6 Split upgrade.rs module | LOW | **GO** | Mechanical `mod` split, no logic changes |
| 4.11a thiserror error enums | HIGH | **PARTIAL** | Add crate + define enum + update UpdateResult; keep `Result<_, String>` on count/list signatures for now |
| 4.10 CommandExecutor trait | HIGH | **PARTIAL** | Phase 1 only: make parser functions `pub(crate)` with unit tests; CommandExecutor trait is DEFER |
| 4.2 BackendKind registry | MEDIUM | **DEFER** | BackendKind is used as event-channel key; full refactor is separate pass |
| glib::clone! macro | LOW | **DEFER** | Cosmetic; no behaviour change; separate pass |
| 4.9 Dead CheckMsg::Error | — | **SKIP** | Already removed per task brief |

---

## Files to Modify

| File | Action |
|------|--------|
| `Cargo.toml` | Add `thiserror = "2"` dependency |
| `src/backends/mod.rs` | Add `BackendError` enum; add `count_available` trait default; update `Backend` trait signatures |
| `src/backends/os_package_manager.rs` | Make parsers `pub(crate)`; remove redundant `count_available` overrides; update `run_update` to return `BackendError` |
| `src/backends/flatpak.rs` | Remove `count_available` override; update `run_update` error type |
| `src/backends/nix.rs` | Update `run_update` error type |
| `src/backends/homebrew.rs` | Remove `count_available` override; update `run_update` error type |
| `src/runner.rs` | Update error string handling to map to `BackendError` |
| `src/upgrade.rs` | Delete `validate_hostname`; reorganise into `src/upgrade/` submodule tree |
| `src/ui/window.rs` | Remove `sort_by_key`; call `UpdateOrchestrator::run()` instead of inline execution |
| `src/ui/upgrade_page.rs` | Add `recompute_state` closure |
| `src/orchestrator.rs` | **NEW** — execution engine extracted from window.rs |
| `src/upgrade/mod.rs` | **NEW** — re-exports from sub-modules |
| `src/upgrade/check.rs` | **NEW** — prerequisite checks |
| `src/upgrade/version.rs` | **NEW** — version arithmetic helpers |
| `src/upgrade/execute.rs` | **NEW** — execute_upgrade and distro-specific runners |
| `src/upgrade/detect.rs` | **NEW** — detect_distro, detect_hostname, detect_nixos_config_type |
| `src/main.rs` | Update module declaration for upgrade submodule (if needed) |

---

## Item 4.12 — Remove Redundant `sort_by_key` in window.rs

### Current State

`src/ui/window.rs`, inside `update_button.connect_clicked`:

```rust
// Reorder: privileged backends first, then unprivileged.
let mut ordered_backends = backends.clone();
ordered_backends.sort_by_key(|b| u8::from(!b.needs_root()));
```

`src/backends/mod.rs`, `detect_backends()` already returns backends in the order:
1. OS package manager (Apt/Dnf/Pacman/Zypper — all `needs_root = true`)
2. Nix (`needs_root = true` on NixOS / Determinate)
3. Flatpak (`needs_root = false`)
4. Homebrew (`needs_root = false`)

### Proposed Change

Delete the `ordered_backends.sort_by_key(...)` call and rename `ordered_backends` back to `backends` (or keep the binding but remove the sort). Change the loop to iterate over `backends` directly.

```rust
// Before
let mut ordered_backends = backends.clone();
ordered_backends.sort_by_key(|b| u8::from(!b.needs_root()));
// ... loop over ordered_backends

// After
let backends_to_run = backends.clone();
// ... loop over backends_to_run
```

### Affected Files

- `src/ui/window.rs`

### Risk

Negligible. Detection order is the authoritative ordering. If the detection order is ever changed, the sort was providing no additional guarantee anyway.

---

## Item 4.11b — Delete Dead `validate_hostname`

### Current State

`src/upgrade.rs` contains:

```rust
/// Validates that a hostname contains only characters safe for use as a NixOS
/// flake output attribute (`[a-zA-Z0-9\-_.]`).
///
/// This mirrors the identical guard in `src/backends/nix.rs`. ...
#[allow(dead_code)]
fn validate_hostname(hostname: &str) -> Result<&str, String> {
    // ...
}
```

It is tagged `#[allow(dead_code)]` and is never called from any code path. The upgrade page uses `glib::markup_escape_text(raw_hostname)` for display only. The actual flake attribute validation is handled entirely by `validate_flake_attr` in `nix.rs` (called via `resolve_nixos_flake_attr`).

### Proposed Change

Delete the entire `validate_hostname` function from `src/upgrade.rs`.

### Affected Files

- `src/upgrade.rs` (or `src/upgrade/detect.rs` after module split)

### Risk

Zero. It is dead code with a suppression annotation.

---

## Item 4.1 — `count_available` Trait Default

### Current State

In `src/backends/mod.rs`, the `Backend` trait has:

```rust
fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
    Box::pin(async { Ok(0) })
}
```

Every backend (Apt, Dnf, Pacman, Zypper, Homebrew) implements `count_available` by running the **same underlying command** as `list_available`, just counting the lines instead of returning them. This is pure duplication.

- `AptBackend`: both run `apt list --upgradable`
- `DnfBackend`: both run `dnf check-update` (with same exit-code handling)
- `PacmanBackend`: both run `pacman -Qu`
- `ZypperBackend`: both run `zypper list-updates`
- `HomebrewBackend`: both run `brew outdated`
- `FlatpakBackend`: `count_available` already delegates to `list_available().map(|v| v.len())`
- `NixBackend`: `count_available` does real work (dry-run / tempdir / determinate-nixd) — must NOT be replaced by the default

### Proposed Change

Change the trait default to delegate to `list_available`:

```rust
// In src/backends/mod.rs — Backend trait
fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
    Box::pin(async move { self.list_available().await.map(|v| v.len()) })
}
```

Remove the explicit `count_available` implementations from:
- `AptBackend`
- `DnfBackend`
- `PacmanBackend`
- `ZypperBackend`
- `HomebrewBackend`
- `FlatpakBackend` (already delegates — now just inherits the default)

**Keep** `NixBackend::count_available` as-is. It performs genuinely distinct logic (dry-run flake checks, determinate-nixd version probing) that cannot be derived from `list_available`.

### Correctness Analysis

For `AptBackend`: `count_available` counted lines containing `/`; `list_available` filters by `/` then takes the first token. `list_available().len()` is identical in count.

For `DnfBackend`: `count_available` counted non-empty non-`Last`-prefixed non-`Obsoleting`-prefixed lines, returning 0 on exit code 0. `list_available` does the same filtering and returns package names. The count would be identical. The exit code 0/100 handling is in `list_available` already (returns `Err` on code 1 only).

For `PacmanBackend`: both use `pacman -Qu`, filter non-empty lines. Identical count.

For `ZypperBackend`: both use `zypper list-updates`, filter lines starting with `v `. Identical count.

For `HomebrewBackend`: both use `brew outdated`, filter non-empty lines. Identical count.

### Affected Files

- `src/backends/mod.rs` (trait default)
- `src/backends/os_package_manager.rs` (remove 5 `count_available` impls)
- `src/backends/flatpak.rs` (remove 1 `count_available` impl)
- `src/backends/homebrew.rs` (remove 1 `count_available` impl)

### Risk

Low. The change does introduce one extra `tokio::process::Command` call per backend check (the `list_available` call returns strings instead of just a count). However, the counts are called in background threads, never in the GTK main loop, so performance impact is negligible. The extra allocation (a `Vec<String>`) is discarded immediately.

---

## Item 4.5 — `recompute_state()` in upgrade_page.rs

### Current State

`src/ui/upgrade_page.rs` has three separate sites that conditionally set `upgrade_button.set_sensitive(...)`:

**Site 1** — in `backup_check.connect_toggled`:
```rust
backup_check.connect_toggled(move |check| {
    if check.is_active()
        && *all_checks_passed_toggled.borrow()
        && *upgrade_available_toggled.borrow()
    {
        upgrade_btn_toggled.set_sensitive(true);
    } else {
        upgrade_btn_toggled.set_sensitive(false);
    }
});
```

**Site 2** — after check results arrive:
```rust
if all_passed && *upgrade_available_ref.borrow() && backup_ref.is_active() {
    upgrade_ref.set_sensitive(true);
} else if !all_passed {
    upgrade_ref.set_sensitive(false);
}
```

**Site 3** — after availability check:
```rust
if !is_available {
    upgrade_btn_for_avail.set_sensitive(false);
}
```

Sites 2 and 3 are inconsistent: Site 2 only enables the button if backup is ticked + all checks passed + available. Site 3 only disables without considering checks or backup state. Site 2 has an asymmetric `else if` that leaves the button enabled if checks fail but only one condition changed.

### Proposed Change

Define a single `recompute_state` closure (captured once, shared across all sites) that evaluates all three conditions:

```rust
// Define once, before wiring up any signals
let recompute_state = {
    let upgrade_button = upgrade_button.clone();
    let upgrade_available = upgrade_available.clone();
    let all_checks_passed = all_checks_passed.clone();
    let backup_check = backup_check.clone();
    Rc::new(move || {
        let enabled = *upgrade_available.borrow()
            && *all_checks_passed.borrow()
            && backup_check.is_active();
        upgrade_button.set_sensitive(enabled);
    })
};
```

Then replace all three sites:

```rust
// Site 1 — backup toggled
let rs = recompute_state.clone();
backup_check.connect_toggled(move |_| rs());

// Site 2 — after check results
*all_checks_passed_ref.borrow_mut() = all_passed;
recompute_state_ref();

// Site 3 — after availability check
*upgrade_available_clone.borrow_mut() = is_available;
recompute_state_avail();
```

### Affected Files

- `src/ui/upgrade_page.rs`

### Risk

Low. Pure refactor of existing logic. Removes the inconsistency at Site 3 (which previously left the button state unchanged if availability check succeeds but checks haven't run).

---

## Item 4.4 — Extract `UpdateOrchestrator`

### Current State

`src/ui/window.rs::build_update_page()` contains, inside `update_button.connect_clicked`, all of:

1. Privilege-check logic (`any_needs_root`)
2. `PrivilegedShell::new()` auth + auth-status channel
3. The backend execution loop (iterating `ordered_backends`, sending `BackendEvent`s)
4. The `event_rx` receive loop that updates GTK widgets (`UpdateRow`, `LogPanel`, `gtk::Label`, `adw::Banner`, `gtk::Button`)

Steps 1–3 are pure execution logic with no GTK dependency. Step 4 is GTK-only.

The key constraint: GTK widgets (`UpdateRow`, `LogPanel`, `gtk::Label`, `adw::Banner`) are `!Send`. They can only be touched on the GTK main thread. The orchestrator **cannot hold widget references**.

### Assessment

The execution engine (steps 1–3) already communicates to the GTK loop via `async_channel::Sender<BackendEvent>`. The logical separation is already there in the channel boundary. The extraction is: move the sender-side logic into a new `src/orchestrator.rs` struct, leaving the receiver-side widget-update code in `window.rs`.

### Proposed Change

**New file: `src/orchestrator.rs`**

```rust
use crate::backends::Backend;
use crate::runner::{BackendEvent, CommandRunner, PrivilegedShell};
use std::sync::Arc;

/// Drives the backend execution pipeline:
/// 1. Optionally authenticates via PrivilegedShell (for backends that need_root)
/// 2. Runs each backend in order, sending BackendEvent to the provided channel
///
/// This struct is Send + Sync. It holds no GTK references.
/// Call `run()` from a background thread / Tokio task.
pub struct UpdateOrchestrator {
    pub backends: Vec<Arc<dyn Backend>>,
    pub event_tx: async_channel::Sender<BackendEvent>,
    /// Sent once: Ok(()) = authenticated (or no root needed), Err(msg) = failed.
    pub auth_tx: async_channel::Sender<Result<(), String>>,
}

impl UpdateOrchestrator {
    pub async fn run(self) {
        let any_needs_root = self.backends.iter().any(|b| b.needs_root());

        let shell: Option<Arc<tokio::sync::Mutex<PrivilegedShell>>> = if any_needs_root {
            match PrivilegedShell::new().await {
                Ok(s) => {
                    let _ = self.auth_tx.send(Ok(())).await;
                    Some(Arc::new(tokio::sync::Mutex::new(s)))
                }
                Err(e) => {
                    let _ = self.auth_tx.send(Err(e)).await;
                    return;
                }
            }
        } else {
            let _ = self.auth_tx.send(Ok(())).await;
            None
        };

        for backend in &self.backends {
            let kind = backend.kind();
            let _ = self.event_tx.send(BackendEvent::Started(kind)).await;
            let runner = CommandRunner::new(self.event_tx.clone(), kind, shell.clone());
            let result = backend.run_update(&runner).await;
            let _ = self.event_tx.send(BackendEvent::Finished(kind, result)).await;
        }

        if let Some(s) = shell {
            s.lock().await.close().await;
        }
    }
}
```

**Modified: `src/ui/window.rs`** — inside `update_button.connect_clicked`:

Replace the `super::spawn_background_async(move || async move { ... })` block (which contains the PrivilegedShell setup and backend loop) with:

```rust
super::spawn_background_async(move || async move {
    let orchestrator = crate::orchestrator::UpdateOrchestrator {
        backends: ordered_backends,
        event_tx: event_tx_thread,
        auth_tx: auth_status_tx,
    };
    orchestrator.run().await;
});
```

The GTK event-receive loop (`while let Ok(event) = event_rx.recv().await { ... }`) remains unchanged in `window.rs`.

### Affected Files

- `src/orchestrator.rs` (new)
- `src/ui/window.rs` (replace inline execution with `UpdateOrchestrator::run()`)
- `src/main.rs` or `src/app.rs` (add `mod orchestrator;`)

### Risk

Medium. The extraction is clean but `PrivilegedShell` has specific Tokio runtime requirements (must be created and used in the same runtime). The existing comment in `window.rs` documents this constraint. The orchestrator's `run()` is `async`, which ensures it stays in the same Tokio runtime as the caller. No runtime boundary is crossed.

### NOT Included (DEFER)

- Moving the event receive loop to a separate `OrchestratorEventHandler` — this would require passing GTK widget handles via some indirection (trait objects or closure maps), which is a larger API change.

---

## Item 4.6 — Split `upgrade.rs` into Submodule Tree

### Current State

`src/upgrade.rs` is ~750 lines containing four conceptually distinct concerns:

1. **Detection** (lines ~1–130): `detect_distro`, `detect_hostname`, `detect_nixos_config_type`, `parse_os_release`, `DistroInfo`, `NixOsConfigType`, `UpgradePageInit`
2. **Prerequisite checks** (lines ~130–300): `run_prerequisite_checks`, `check_packages_up_to_date`, `check_disk_space`, `check_nixos_rebuild_available`, `CheckResult`
3. **Version arithmetic / availability checks** (lines ~300–590): `check_upgrade_available`, `check_ubuntu_upgrade`, `check_fedora_upgrade`, `check_opensuse_upgrade`, `check_nixos_upgrade`, `next_nixos_channel`, `next_opensuse_leap_version`, `parse_ubuntu_version`, `parse_meta_release_for_upgrade`, `UbuntuUpgradeInfo`, `read_upgrade_prompt_policy`, `fetch_ubuntu_meta_release`, `check_ubuntu_upgrade_via_tool`
4. **Execution** (lines ~590–750): `execute_upgrade`, `upgrade_ubuntu`, `upgrade_fedora`, `upgrade_opensuse`, `upgrade_nixos`

### Proposed Change

Convert to a module tree:

```
src/upgrade/
  mod.rs         — re-exports everything consumed by callers; contains shared types
  detect.rs      — detect_distro, detect_hostname, detect_nixos_config_type, parse_os_release
  check.rs       — run_prerequisite_checks, check_packages_up_to_date, check_disk_space, CheckResult
  version.rs     — check_upgrade_available, check_ubuntu_upgrade, UbuntuUpgradeInfo, etc.
  execute.rs     — execute_upgrade, upgrade_ubuntu, upgrade_fedora, upgrade_opensuse, upgrade_nixos
```

`mod.rs` re-exports all types that external code currently imports via `crate::upgrade::*`:

```rust
// src/upgrade/mod.rs
pub mod check;
pub mod detect;
pub mod execute;
pub mod version;

pub use check::{run_prerequisite_checks, CheckResult};
pub use detect::{
    detect_distro, detect_hostname, detect_nixos_config_type, DistroInfo, NixOsConfigType,
    UpgradePageInit,
};
pub use execute::execute_upgrade;
pub use version::{check_upgrade_available, next_nixos_channel, UbuntuUpgradeInfo};
```

Callers (`src/ui/window.rs`, `src/ui/upgrade_page.rs`) use `crate::upgrade::*` — no import changes needed.

The `validate_hostname` dead function is deleted from `detect.rs` (implementing Item 4.11b simultaneously).

### Affected Files

- `src/upgrade.rs` → deleted / renamed to `src/upgrade/mod.rs`
- `src/upgrade/detect.rs` (new)
- `src/upgrade/check.rs` (new)
- `src/upgrade/version.rs` (new)
- `src/upgrade/execute.rs` (new)

### Risk

Low. Purely mechanical. No logic changes. All existing `use crate::upgrade::` imports continue to work via re-exports.

---

## Item 4.11a — `thiserror` Error Enums

### Current State

`thiserror` is **not** in `Cargo.toml`. All error paths return `String`:

- `Backend::run_update` → `UpdateResult::Error(String)`
- `Backend::count_available` → `Result<usize, String>`
- `Backend::list_available` → `Result<Vec<String>, String>`
- `CommandRunner::run` → `Result<String, String>`
- `PrivilegedShell::run_command` → `Result<String, String>`

With `String` errors, distinguishing auth cancellation from network failure from command failure requires fragile string-matching (e.g., `e.contains("authentication was cancelled")`).

### Proposed Change (PARTIAL)

#### Step 1: Add `thiserror` to `Cargo.toml`

```toml
[dependencies]
# ... existing ...
thiserror = "2"
```

#### Step 2: Define `BackendError` in `src/backends/mod.rs`

```rust
use thiserror::Error;

#[derive(Debug, Error, Clone)]
pub enum BackendError {
    /// pkexec exited with code 126 (auth cancelled) or 127 (not authorised).
    #[error("Authentication cancelled or denied")]
    AuthCancelled,

    /// The command could not be spawned (binary not found, permission error).
    #[error("Failed to spawn process: {0}")]
    Spawn(String),

    /// The command was spawned but exited with a non-zero status code.
    #[error("Command failed (exit {code}): {message}")]
    Exit { code: i32, message: String },

    /// Output from the command could not be parsed.
    #[error("Failed to parse command output: {0}")]
    Parse(String),

    /// A network operation (curl, HTTP check) failed.
    #[error("Network error: {0}")]
    Network(String),
}

impl BackendError {
    /// Convert a raw error string (from the current String-based API) into the
    /// most specific BackendError variant. Used as a bridge during migration.
    pub fn from_string(s: String) -> Self {
        let lower = s.to_ascii_lowercase();
        if lower.contains("authentication was cancelled")
            || lower.contains("not authorised")
            || s.contains("exit code 126")
            || s.contains("exit code 127")
        {
            return BackendError::AuthCancelled;
        }
        if lower.contains("failed to start") || lower.contains("no such file or directory") {
            return BackendError::Spawn(s);
        }
        if lower.contains("exited with code") {
            // Try to parse exit code
            let code = s
                .split("code ")
                .nth(1)
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|n| n.parse::<i32>().ok())
                .unwrap_or(-1);
            return BackendError::Exit { code, message: s };
        }
        BackendError::Exit { code: -1, message: s }
    }
}
```

#### Step 3: Update `UpdateResult`

```rust
// src/backends/mod.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UpdateResult {
    Success { updated_count: usize },
    SuccessWithSelfUpdate { updated_count: usize },
    /// Structured error replacing the plain String variant.
    Error(BackendError),
    Skipped(String),
}
```

Since `BackendError` derives `Clone` and the Serde requirement only matters for persistence (not currently used), add `#[serde(skip)]` or implement serialize/deserialize for `BackendError` if needed. For the current codebase (no serialization of `UpdateResult` to disk), simply derive `Serialize, Deserialize` on `BackendError` with string fallback using `#[serde(tag = "type")]`.

Actually, since `UpdateResult` is not serialized to disk in the current codebase (it's used only in memory via channels), we can remove the `Serialize, Deserialize` derives from `UpdateResult` entirely, or keep them with the new error type. Simplest: add `#[serde(skip_serializing_if = "is_never")]` workaround, or just impl Serialize manually. **Recommendation**: Remove Serialize/Deserialize from UpdateResult for this pass (it's used only in the async channel, never persisted).

#### Step 4: Update `CommandRunner::run` and `PrivilegedShell::run_command`

`runner.rs` currently returns `Result<String, String>`. The errors it produces are used exclusively by backend `run_update` methods. Update `CommandRunner::run` to return `Result<String, BackendError>`:

```rust
// src/runner.rs
use crate::backends::BackendError;

impl CommandRunner {
    pub async fn run(&self, program: &str, args: &[&str]) -> Result<String, BackendError> {
        // ...spawn error:
        .map_err(|e| BackendError::Spawn(format!("Failed to start {program}: {e}")))?;
        
        // ...non-zero exit:
        Err(BackendError::Exit { code, message: format!("{program} exited with code {code}") })
    }
}
```

`PrivilegedShell::run_command` returns `Result<String, String>` (internal). Keep it as `String` internally; the translation to `BackendError` happens in `CommandRunner::run` (which wraps the shell call).

Actually `CommandRunner::run` calls `guard.run_command(args, &self.tx, self.kind).await` and maps its `Err`. We translate at the `CommandRunner` boundary.

#### Step 5: Update all backends

Each `run_update` that currently does:
```rust
Err(e) => UpdateResult::Error(e),
```
becomes:
```rust
Err(e) => UpdateResult::Error(e), // e is now BackendError — no change needed at call site
```

Because `CommandRunner::run` now returns `Result<String, BackendError>`, the `Err(e)` in `run_update` is already a `BackendError`. No change to backend call sites needed.

#### What is NOT changed in this pass

- `Backend::count_available` signature stays `Result<usize, String>` — the UI currently only displays the error as a string via `row.set_status_unknown(&msg)`. Converting would require updating all call sites plus the `UpdateRow` widget. **DEFER** to a follow-on pass.
- `Backend::list_available` signature stays `Result<Vec<String>, String>` — same reason.

### Affected Files

- `Cargo.toml` (add `thiserror = "2"`)
- `src/backends/mod.rs` (add `BackendError`; update `UpdateResult`)
- `src/runner.rs` (update `CommandRunner::run` return type)
- `src/backends/os_package_manager.rs` (update `Err(e)` patterns — no site changes needed since `CommandRunner::run` now returns `BackendError`)
- `src/backends/flatpak.rs` (same)
- `src/backends/nix.rs` (same)
- `src/backends/homebrew.rs` (same)

### Risk

Medium. `UpdateResult` is used in `window.rs` for match arms. The `Error(String)` → `Error(BackendError)` change requires updating the match in `window.rs`:

```rust
// Before
UpdateResult::Error(msg) => {
    row.set_status_error(msg);
    has_error = true;
}
// After
UpdateResult::Error(e) => {
    row.set_status_error(&e.to_string());
    has_error = true;
}
```

`BackendError` implements `Display` via `thiserror`, so `.to_string()` gives the same human-readable string as before. This is a minimal change at the UI layer.

---

## Item 4.10 — CommandExecutor (PARTIAL: Parsers Only)

### Current State

Parser functions in `src/backends/os_package_manager.rs` are private:

```rust
fn count_apt_upgraded(output: &str) -> usize { ... }
fn count_dnf_upgraded(output: &str) -> usize { ... }
```

These encode non-trivial parsing logic (APT "N upgraded", DNF "Upgrade  N Packages", Pacman line counting) but have zero test coverage because they are private.

Similar private parsers in `nix.rs`:
- `count_nix_store_operations(output: &str) -> usize`
- `compare_lock_nodes(old: &Value, new: &Value) -> Vec<String>`
- `upgrade_available_in_output(output: &str) -> bool`
- `count_determinate_upgraded(output: &str) -> usize`

### Phase 1 (GO — in this pass)

Change visibility to `pub(crate)` for all parser functions:

```rust
// os_package_manager.rs
pub(crate) fn count_apt_upgraded(output: &str) -> usize { ... }
pub(crate) fn count_dnf_upgraded(output: &str) -> usize { ... }
pub(crate) fn count_pacman_upgraded(output: &str) -> usize { ... } // extract inline count
pub(crate) fn count_zypper_upgraded(output: &str) -> usize { ... } // extract inline count
pub(crate) fn count_homebrew_upgraded(output: &str) -> usize { ... } // extract inline count

// nix.rs
pub(crate) fn count_nix_store_operations(output: &str) -> usize { ... }
pub(crate) fn upgrade_available_in_output(output: &str) -> bool { ... }
pub(crate) fn count_determinate_upgraded(output: &str) -> usize { ... }
pub(crate) fn compare_lock_nodes(old: &serde_json::Value, new: &serde_json::Value) -> Vec<String> { ... }
```

Then add unit tests in a `#[cfg(test)]` module at the bottom of each file:

```rust
// os_package_manager.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_apt_upgraded_normal() {
        let output = "0 upgraded, 0 newly installed, 0 to remove and 0 not upgraded.";
        assert_eq!(count_apt_upgraded(output), 0);
    }

    #[test]
    fn test_count_apt_upgraded_some() {
        let output = "3 upgraded, 0 newly installed, 0 to remove and 1 not upgraded.";
        assert_eq!(count_apt_upgraded(output), 3);
    }

    #[test]
    fn test_count_dnf_upgraded_dnf4() {
        let output = "  Upgrade  15 Packages\n\nTransaction Summary";
        assert_eq!(count_dnf_upgraded(output), 15);
    }

    #[test]
    fn test_count_dnf_upgraded_dnf5() {
        let output = "  Upgrading: 7 packages";
        assert_eq!(count_dnf_upgraded(output), 7);
    }

    // ... more tests for pacman, zypper, homebrew parsers
}
```

```rust
// nix.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_nix_store_ops_zero() {
        assert_eq!(count_nix_store_operations("nothing to do"), 0);
    }

    #[test]
    fn test_count_nix_store_ops_build_and_fetch() {
        let output = "these 2 derivations will be built:\nthese 5 paths will be fetched (10 MiB download, 50 MiB unpacked):";
        assert_eq!(count_nix_store_operations(output), 7);
    }

    #[test]
    fn test_upgrade_available_in_output() {
        assert!(upgrade_available_in_output("An upgrade is available for your system"));
        assert!(!upgrade_available_in_output("Already on the latest version"));
    }
}
```

### Phase 2 (DEFER)

Introduce `trait CommandExecutor` to abstract `tokio::process::Command` in `count_available` and `list_available`. This requires:
- A `trait CommandExecutor: Send + Sync` with `async fn run_unprivileged(...) -> Result<Output, io::Error>`
- A `RealExecutor` that wraps `tokio::process::Command`
- A `MockExecutor` for tests
- Threading the executor through all backend structs

This is a **larger, self-contained pass** best done after the parser tests give a safety net. DEFER to a follow-on architecture ticket.

### Affected Files (Phase 1 only)

- `src/backends/os_package_manager.rs` (change visibility; add tests; extract inline parse logic to named `pub(crate) fn`)
- `src/backends/nix.rs` (change visibility; add tests)
- `src/backends/homebrew.rs` (change visibility; add tests)
- `src/backends/flatpak.rs` (extract and expose any inline parse logic; add tests)

---

## Item 4.2 — BackendKind Registry (DEFER)

### Assessment

`BackendKind` is used as the key for the `BackendEvent` channel protocol (`BackendEvent::Started(BackendKind)`, `BackendEvent::LogLine(BackendKind, String)`, `BackendEvent::Finished(BackendKind, UpdateResult)`). It is also used in `window.rs` as the key for `rows` lookup: `rows_borrowed.iter().find(|(k, _)| *k == kind)`.

A full registry pattern (removing `BackendKind` entirely and identifying backends by index or UUID) would require:
1. Replacing `BackendKind` in `BackendEvent` with a numeric ID or `usize` index
2. Updating `CommandRunner` to accept the new ID
3. Updating the `rows` Vec from `Vec<(BackendKind, UpdateRow)>` to `Vec<(usize, UpdateRow)>`
4. Updating all `Display` and `Debug` impls that currently use `BackendKind` for logging

This is a significant cross-cutting refactor. **DEFER** to a dedicated pass after the other items stabilise.

The `sort_by_key` removal (Item 4.12) is the low-risk improvement for this area.

---

## glib::clone! Macro (DEFER)

The verbose `Rc::clone()` patterns in `window.rs` and `upgrade_page.rs` are cosmetic. The `glib::clone!` macro would reduce boilerplate but changes no semantics. **DEFER** to a dedicated cleanup pass.

---

## Implementation Order (Recommended)

To minimise compile-break risk, implement in this order:

1. **Item 4.12** — Remove sort (2 lines deleted)
2. **Item 4.11b** — Delete `validate_hostname` (function deleted)
3. **Item 4.6** — Split `upgrade.rs` into submodule tree (mechanical move, re-exports preserve API)
4. **Item 4.1** — Trait default for `count_available` (removes duplicate `count_available` impls)
5. **Item 4.11a** — Add `thiserror`, `BackendError`, update `UpdateResult` and `CommandRunner`
6. **Item 4.10** — Make parser functions `pub(crate)`, add unit tests
7. **Item 4.4** — Extract `UpdateOrchestrator`
8. **Item 4.5** — `recompute_state()` in upgrade_page.rs

Steps 1–4 are independent and can proceed in parallel. Steps 5–6 are independent of each other. Step 7 depends on the codebase compiling cleanly after steps 5.

---

## New Crate Dependencies

| Crate | Version | Added to | Purpose |
|-------|---------|----------|---------|
| `thiserror` | `"2"` | `[dependencies]` in `Cargo.toml` | Derive macros for structured error enums |

No other new crates are required. All other changes use existing crates (`glib`, `adw`, `gtk`, `async-channel`, `tokio`, `serde_json`).

---

## Acceptance Criteria

After implementation:

- `cargo build` — zero errors, zero warnings
- `cargo clippy -- -D warnings` — zero warnings
- `cargo fmt --check` — no formatting diffs
- `cargo test` — all parser unit tests pass (new tests for count_apt_upgraded, count_dnf_upgraded, count_nix_store_operations, etc.)
- `window.rs` no longer contains the backend execution loop (PrivilegedShell setup + backend loop moved to `src/orchestrator.rs`)
- `upgrade.rs` file no longer exists; `src/upgrade/` directory exists with 4 submodules
- `BackendError` enum is importable from `crate::backends::BackendError`
- `UpdateResult::Error` holds a `BackendError` not a `String`
- All parser functions referenced in unit tests are `pub(crate)` not private
- `sort_by_key` call is absent from `window.rs`
- `validate_hostname` is absent from `upgrade.rs` (or its replacement submodule)
