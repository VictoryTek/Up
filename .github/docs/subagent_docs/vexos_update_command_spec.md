# VexOS Update Command Integration — Specification

**Feature:** VexOS update command replacement  
**Spec file:** `.github/docs/subagent_docs/vexos_update_command_spec.md`  
**Date:** 2026-05-24  

---

## 1. Current State Analysis

### 1.1 VexOS Detection

There is no `is_vexos()` function in the codebase. VexOS is a NixOS flake-based variant, so the current code path for a VexOS system is:

1. `is_nixos()` → `true` (reads `/run/current-system` or `/etc/os-release`)
2. `is_nixos_flake()` → `true` (`/etc/nixos/flake.nix` exists)
3. `resolve_nixos_flake_attr()` → reads `/etc/nixos/vexos-variant` to get the config attribute

The only VexOS-specific file referenced is `/etc/nixos/vexos-variant`. Its **presence** is the reliable VexOS indicator.

### 1.2 Full Update Path (Step 6 — the path being changed)

File: `src/backends/nix.rs`  
Function: `NixBackend::run_update()`  
Branch: `if is_nixos() { if is_nixos_flake() { ... } }`

Current command constructed (lines ~453–469):

```rust
let cmd = format!(
    "stdbuf -oL -eL \
     nix --extra-experimental-features 'nix-command flakes' \
     flake update --flake /etc/nixos && \
     stdbuf -oL -eL \
     nixos-rebuild switch --flake /etc/nixos#{} --refresh --print-build-logs",
    config_name
);
runner.run(
    "pkexec",
    &[
        "env",
        "PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin",
        "sh",
        "-c",
        &cmd,
    ],
).await
```

Result mapping:
- `Ok(output)` → `UpdateResult::Success { updated_count: count_nix_store_operations(&output) }`
- `Err(e)` → `UpdateResult::Error(e)`

### 1.3 Partial Update Path (run_selected_update — PRESERVE AS-IS)

File: `src/backends/nix.rs`  
Function: `NixBackend::run_selected_update()`

```rust
let cmd = format!(
    "stdbuf -oL -eL \
     nix --extra-experimental-features 'nix-command flakes' \
     flake update {} --flake /etc/nixos && \
     stdbuf -oL -eL \
     nixos-rebuild switch --flake /etc/nixos#{} --refresh --print-build-logs",
    inputs_str, config_name
);
```

This path is **not changed** by this feature. VexOS does not use per-input selection.

### 1.4 Item Selection Support

Function: `NixBackend::supports_item_selection()`

```rust
fn supports_item_selection(&self) -> bool {
    is_nixos() && is_nixos_flake()
}
```

Currently returns `true` for any NixOS flake system, including VexOS. Since VexOS uses a single wrapper command that handles everything internally, per-input selection is inapplicable on VexOS. **This must be disabled for VexOS.**

### 1.5 Exit Code Flow

Exit codes flow as follows:

1. `PrivilegedShell::run_command()` reads a sentinel line to detect exit code.  
   On non-zero exit: returns `Err(format!("Command exited with code {code}"))`.

2. `CommandRunner::run()` (for pkexec, routes to `PrivilegedShell::run_command()`)  
   calls `BackendError::from_string()` on the error, which parses the exit code string:
   ```rust
   if lower.contains("exited with code") {
       let code = ...parse...;
       return BackendError::Exit { code, message: s };
   }
   ```
   So exit code 2 from `vexos-update` produces:  
   `Err(BackendError::Exit { code: 2, message: "Command exited with code 2" })`

3. `run_update()` in `nix.rs` matches `Err(e) => UpdateResult::Error(e)`.

4. `orchestrator.rs` sends `OrchestratorEvent::BackendFinished(kind, result)` to the UI.

5. `window.rs` matches `UpdateResult::Error(msg)` → calls `row.set_status_error(...)`.

### 1.6 UpdateResult Variants (current)

File: `src/backends/mod.rs`

```rust
pub enum UpdateResult {
    Success { updated_count: usize },
    SuccessWithSelfUpdate { updated_count: usize },
    Error(BackendError),
    Skipped(String),
    Cancelled,
}
```

There is no "soft" non-error terminal state for a cache miss / hold condition.

### 1.7 UI Status Methods (UpdateRow)

File: `src/ui/update_row.rs`

| Method | CSS classes | Retry button |
|---|---|---|
| `set_status_error(msg)` | `["error"]` | visible |
| `set_status_success(n)` | `["success"]` | hidden |
| `set_status_skipped(msg)` | `["dim-label"]` | hidden |
| `set_status_cancelled()` | `["dim-label"]` | hidden |
| `set_status_unknown(msg)` | `["dim-label"]` | hidden |

There is no "on hold" / "cache miss" status display method.

### 1.8 window.rs BackendFinished Handling

Two `BackendFinished` match arms exist:
- Line ~836: inside the update orchestrator event loop (the main update path)
- Line ~303: inside the maintenance/cleanup orchestrator event loop

Both must handle `UpdateResult::CacheMiss`.

History recording also matches `UpdateResult` variants. `CacheMiss` must be added.

---

## 2. Problem Definition

When `vexos-update` exits with code 2, it means the binary cache has not yet built the requested packages ("cache miss / updates on hold"). This is a normal, expected, non-error state. The current code would treat this as `UpdateResult::Error(BackendError::Exit { code: 2, ... })` and display an error row with a red "Error: Command exited with code 2" label and a Retry button, which is incorrect and confusing to the user.

Additionally, the VexOS team has replaced the two-step update process (`nix flake update` + `nixos-rebuild switch`) with a single `vexos-update` wrapper that handles everything. The old command must no longer be used on VexOS systems.

---

## 3. Proposed Solution Architecture

### 3.1 New `is_vexos()` Function

Add to `src/backends/nix.rs`:

```rust
/// True when running on VexOS (a NixOS variant).
///
/// Detection: presence of `/etc/nixos/vexos-variant`, a mandatory file
/// created during VexOS configuration to record the active variant name.
fn is_vexos() -> bool {
    if crate::backends::flatpak::is_running_in_flatpak() {
        return std::process::Command::new("flatpak-spawn")
            .args(["--host", "test", "-e", "/etc/nixos/vexos-variant"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    }
    std::path::Path::new("/etc/nixos/vexos-variant").exists()
}
```

**Placement:** after `is_nixos_flake()` (line ~50), before `validate_flake_attr()`.

### 3.2 New `UpdateResult::CacheMiss` Variant

Add to the `UpdateResult` enum in `src/backends/mod.rs`:

```rust
pub enum UpdateResult {
    Success { updated_count: usize },
    SuccessWithSelfUpdate { updated_count: usize },
    Error(BackendError),
    Skipped(String),
    Cancelled,
    /// The update tool exited with a "cache miss" code (exit 2 on VexOS).
    /// Updates are on hold while the binary cache catches up; this is not an error.
    CacheMiss,
}
```

### 3.3 Modified `run_update()` in NixBackend

Inside the `if is_nixos() { if is_nixos_flake() { ... } }` branch, add a VexOS-specific sub-branch **before** the existing standard NixOS flake command:

**Pseudocode / description:**

```
if is_nixos() {
    if is_nixos_flake() {
        if is_vexos() {
            // VexOS path: single wrapper script, no nix flake update, no nixos-rebuild
            match runner.run(
                "pkexec",
                &[
                    "env",
                    "PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin",
                    "sh",
                    "-c",
                    "stdbuf -oL -eL vexos-update",
                ],
            ).await {
                Ok(output) => UpdateResult::Success {
                    updated_count: count_nix_store_operations(&output),
                },
                Err(BackendError::Exit { code: 2, .. }) => UpdateResult::CacheMiss,
                Err(e) => UpdateResult::Error(e),
            }
        } else {
            // Standard NixOS flake path (existing code — unchanged)
            let config_name = match resolve_nixos_flake_attr() { ... };
            let cmd = format!(
                "stdbuf -oL -eL nix ... flake update --flake /etc/nixos && \
                 stdbuf -oL -eL nixos-rebuild switch --flake /etc/nixos#{config_name} ..."
            );
            runner.run("pkexec", ...).await
            // Ok → Success, Err → Error (unchanged)
        }
    }
    // ... legacy channel path unchanged
}
```

**Key implementation note:** The `Err(BackendError::Exit { code: 2, .. })` pattern match uses struct field destructuring with `..` to match regardless of the `message` field value. The `code: 2` binding is a literal pattern. This is idiomatic Rust pattern matching on the `BackendError::Exit { code, message }` variant.

### 3.4 Modified `supports_item_selection()` in NixBackend

Change from:
```rust
fn supports_item_selection(&self) -> bool {
    is_nixos() && is_nixos_flake()
}
```

To:
```rust
fn supports_item_selection(&self) -> bool {
    is_nixos() && is_nixos_flake() && !is_vexos()
}
```

This prevents the UI from showing per-input checkboxes on VexOS (since `vexos-update` handles all inputs internally and `run_selected_update` must not be called).

### 3.5 New `set_status_on_hold()` in UpdateRow

Add to `src/ui/update_row.rs`:

```rust
/// Display a "cache miss / updates on hold" status.
/// Used when VexOS's vexos-update exits with code 2.
/// Styled as a warning (neutral, not an error).
pub fn set_status_on_hold(&self) {
    self.retry_button.set_visible(false);
    self.skip_checkbox.set_sensitive(true);
    self.spinner.set_visible(false);
    self.spinner.set_spinning(false);
    self.status_label
        .set_label(&gettext("Updates on hold \u{2014} cache catching up"));
    self.status_label.set_css_classes(&["warning"]);
}
```

**CSS class:** `"warning"` — libadwaita ships a `warning` named color (`@warning_color`) which is distinct from both `"error"` (red) and `"success"` (green). This conveys "non-error but noteworthy" correctly.

**i18n string:** `"Updates on hold — cache catching up"` (uses em-dash `\u{2014}`). Wrapped in `gettext()` so it participates in the translation pipeline.

### 3.6 Modified `BackendFinished` Handlers in window.rs

#### 3.6.1 Main update orchestrator (line ~836)

In the `UpdateResult` match arm:

```rust
UpdateResult::CacheMiss => {
    row.set_status_on_hold();
}
```

Since `CacheMiss` is a non-error soft state:
- `has_error` flag is **not** set
- The status bar message at the end of `AllFinished` can remain "Update complete." (cache miss is not an error)

#### 3.6.2 History recording (line ~864)

Add a `CacheMiss` arm to the history match:

```rust
UpdateResult::CacheMiss => crate::history::HistoryEntry {
    timestamp: ts,
    backend: kind.to_string(),
    result: "cache_miss".to_string(),
    updated_count: None,
    error: None,
},
```

The existing `if !matches!(result, UpdateResult::Cancelled)` guard at line ~864 already excludes `Cancelled` from history. `CacheMiss` must be included in history recording (it is a meaningful terminal state worth logging).

#### 3.6.3 Maintenance cleanup orchestrator (line ~303)

The maintenance path also matches `UpdateResult`. Add:

```rust
UpdateResult::CacheMiss => {
    row.set_status_on_hold();
}
```

(Though `CacheMiss` would never be emitted by non-VexOS backends during maintenance, the match must be exhaustive.)

### 3.7 VEXOS_CACHE_MISS Line Prefix — Log Annotation (Supplementary)

The VexOS integration requirement also mentions detecting lines with the `VEXOS_CACHE_MISS:` prefix. These flow as `OrchestratorEvent::BackendLog(BackendKind::Nix, line)` events.

The exit code 2 is the authoritative signal for the final `CacheMiss` result. The line prefix is supplementary diagnostic output that may appear in the streaming log before the command exits.

**Approach:** In `window.rs`, in the `BackendLog` handler for the main update orchestrator, detect the `VEXOS_CACHE_MISS:` prefix specifically for `BackendKind::Nix` and forward the message body (after the prefix) to the log panel with a distinct label:

```rust
OrchestratorEvent::BackendLog(kind, line) => {
    if kind == BackendKind::Nix && line.starts_with("VEXOS_CACHE_MISS:") {
        let msg = line.trim_start_matches("VEXOS_CACHE_MISS:").trim();
        log_panel.append_line(&format!("[Nix] \u{26A0} {msg}"));
    } else {
        log_panel.append_line(&format!("[{kind}] {line}"));
    }
}
```

This is a **non-critical supplementary enhancement**. If the implementation is time-constrained, this can be deferred. The core correctness requirement is exit code 2 → `CacheMiss`.

---

## 4. What NOT to Change

| Item | Reason |
|---|---|
| `run_selected_update()` in `nix.rs` | VexOS requirement explicitly states partial update path is unchanged |
| Legacy NixOS channel path (non-flake) | Only flake-based VexOS is affected |
| Non-NixOS Nix paths (profile, determinate) | Unrelated |
| `resolve_nixos_flake_attr()` | Still called by `run_selected_update()` for standard NixOS; also used on VexOS for `run_selected_update()` if ever called |
| `count_nix_store_operations()` | Still applicable for parsing `vexos-update` output |
| `is_nixos_activation_success()` in `runner.rs` | NixOS shell close detection is still relevant if `vexos-update` internally calls `nixos-rebuild` |
| `BackendError` variants | No new error types needed; exit code 2 is handled at the `UpdateResult` level |

---

## 5. Implementation Steps

1. **`src/backends/nix.rs`** — Add `is_vexos()` after `is_nixos_flake()`.

2. **`src/backends/mod.rs`** — Add `CacheMiss` variant to `UpdateResult` with doc comment.

3. **`src/backends/nix.rs`** — Modify `run_update()`: wrap existing flake body in `if !is_vexos() { ... } else { ... }` where the `else` block runs `vexos-update` and maps exit 2 to `CacheMiss`.

4. **`src/backends/nix.rs`** — Modify `supports_item_selection()`: add `&& !is_vexos()`.

5. **`src/ui/update_row.rs`** — Add `set_status_on_hold()` method.

6. **`src/ui/window.rs`** — Handle `UpdateResult::CacheMiss` in both `BackendFinished` match arms and in history recording. Add `VEXOS_CACHE_MISS:` log annotation in `BackendLog` handler.

7. **Verify exhaustive match**: After adding `CacheMiss`, `cargo build` will error on non-exhaustive match arms anywhere `UpdateResult` is matched. Find all match sites and add the `CacheMiss` arm. Known sites:
   - `src/ui/window.rs` (two `BackendFinished` handlers, one history recorder)
   - Any other code that pattern-matches `UpdateResult` (run `grep -rn "UpdateResult::" src/` to confirm)

---

## 6. Dependencies

No new Cargo dependencies required. All changes are internal.

---

## 7. Risks and Mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| `vexos-update` binary not on PATH inside pkexec environment | Low | The `PATH=` override in the pkexec invocation includes `/run/current-system/sw/bin` which is where NixOS-managed binaries live. VexOS must place `vexos-update` there. |
| `BackendError::Exit { code: 2 }` parse regression | Low | `BackendError::from_string()` reliably parses "exited with code N" strings. The `code: 2` literal pattern in `Err(BackendError::Exit { code: 2, .. })` is a compile-time constant match — no string fragility. |
| `is_vexos()` false positive on non-VexOS NixOS with a hand-created `vexos-variant` file | Very low | Users who manually create `/etc/nixos/vexos-variant` are advanced users who understand the implication; treat this as acceptable. |
| `CacheMiss` not exhaustively matched | Certain (compile error) | `cargo build` will catch this immediately. The implementation step explicitly lists all match sites. |
| `"warning"` CSS class not rendering as expected | Low | libadwaita's named color `@warning_color` is part of its stable design system since v1.0. The `warning` CSS class is used elsewhere in Adwaita-based apps. |
| `is_vexos()` called multiple times per update run (minor perf) | Very low | Function is a single `Path::new(...).exists()` call — negligible. Can be cached with `std::sync::OnceLock` in the future if profiling warrants. |
| Shell close during `vexos-update` (nixos activation kills pkexec) | Medium | `is_nixos_activation_success()` in `runner.rs` already handles this case for NixOS rebuilds. If `vexos-update` internally runs `nixos-rebuild switch`, the existing activation markers will correctly be detected and the result will be treated as success. |

---

## 8. Affected File Summary

| File | Change type |
|---|---|
| `src/backends/nix.rs` | Add `is_vexos()`, modify `run_update()`, modify `supports_item_selection()` |
| `src/backends/mod.rs` | Add `UpdateResult::CacheMiss` variant |
| `src/ui/update_row.rs` | Add `set_status_on_hold()` method |
| `src/ui/window.rs` | Handle `CacheMiss` in 3 match sites + log annotation |

---

## 9. Test Considerations

- Add unit test `run_update_vexos_cache_miss_returns_cache_miss` using `MockExecutor` that simulates exit code 2 on VexOS, asserting the result is `UpdateResult::CacheMiss`.
- Add unit test `run_update_vexos_success` asserting exit 0 maps to `UpdateResult::Success`.
- Add unit test `supports_item_selection_false_on_vexos` asserting `false` when VexOS is detected.
- These belong in the existing `#[cfg(test)]` block in `src/backends/nix.rs`.
