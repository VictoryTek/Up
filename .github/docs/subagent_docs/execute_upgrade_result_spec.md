# Specification: `execute_upgrade()` Returns `Result<(), String>` Instead of `bool`

**Feature Name:** `execute_upgrade_result`  
**Tracking Finding:** #10 — `execute_upgrade()` returns `bool` instead of `Result<(), String>`  
**Date:** 2026-03-19  
**Status:** Draft  

---

## 1. Current State Analysis

### 1.1 Exact Signature

```rust
// src/upgrade.rs, line 373
pub fn execute_upgrade(distro: &DistroInfo, tx: &async_channel::Sender<String>) -> bool
```

### 1.2 Helper Function Signatures (all return `bool`)

```rust
fn upgrade_ubuntu(tx: &async_channel::Sender<String>) -> bool
fn upgrade_fedora(tx: &async_channel::Sender<String>) -> bool
fn upgrade_opensuse(tx: &async_channel::Sender<String>) -> bool
fn upgrade_nixos(tx: &async_channel::Sender<String>) -> bool
```

### 1.3 All Early-Return `false` Paths

The following table enumerates every point where `false` can be returned across
`execute_upgrade` and its four delegate functions.

| # | Location | Function | Trigger | Current behaviour |
|---|----------|----------|---------|------------------|
| 1 | `execute_upgrade`, `_ =>` arm | `execute_upgrade` | Distro not in `"ubuntu"\|"debian"\|"fedora"\|"opensuse-leap"\|"nixos"` | Sends message via `tx`; returns `false` |
| 2 | `upgrade_ubuntu` | `run_command_sync` | `do-release-upgrade -f DistUpgradeViewNonInteractive` exits non-zero or fails to spawn | `run_command_sync` sends error via `tx`; returns `false`; propagated directly |
| 3 | `upgrade_fedora` step 1 | `run_command_sync` | `pkexec dnf install -y dnf-plugin-system-upgrade` fails | Sends error via `tx`; `return false` |
| 4 | `upgrade_fedora` step 2a | `detect_next_fedora_version()` returns `None` | rpm/os-release parse fails | Sends "Could not detect current Fedora version. Aborting upgrade." via `tx`; `return false` |
| 5 | `upgrade_fedora` step 2b | `run_command_sync` | `pkexec dnf system-upgrade download --releasever N -y` fails | Sends error via `tx`; `return false` |
| 6 | `upgrade_fedora` step 3 | `run_command_sync` | `pkexec dnf system-upgrade reboot` fails | Sends error via `tx`; return value propagated directly |
| 7 | `upgrade_opensuse` | `run_command_sync` | `pkexec zypper dup -y` fails | Sends error via `tx`; return value propagated directly |
| 8 | `upgrade_nixos` (legacy) step 1 | `run_command_sync` | `pkexec sh -c '...nix-channel --update'` fails | Sends error via `tx`; `return false` |
| 9 | `upgrade_nixos` (legacy) step 2 | `run_command_sync` | `pkexec nixos-rebuild switch --upgrade` fails | Sends error via `tx`; return value propagated directly |
| 10 | `upgrade_nixos` (flake) step 1 | `run_command_sync` | `pkexec sh -c '...nix flake update --flake /etc/nixos'` fails | Sends error via `tx`; `return false` |
| 11 | `upgrade_nixos` (flake) step 2 | `validate_hostname` | Hostname invalid (empty, too long, illegal chars) | Sends "Upgrade aborted: {e}" via `tx`; `return false` |
| 12 | `upgrade_nixos` (flake) step 3 | `run_command_sync` | `pkexec nixos-rebuild switch --flake /etc/nixos#{host}` fails | Sends error via `tx`; return value propagated directly |

> **Observation:** For paths 2, 6, 7, 9, 12 the error message is already
> streamed via `tx` by `run_command_sync` (exit code, stderr, or spawn error).
> The `bool` is returned but the *reason* is already visible in the log panel.
> For path 11, the message is sent via `tx` right before `return false`.
> For path 1, 4 the message is sent via `tx` before `return false`.
> Despite the messages existing in the log, there is no single consolidated
> "Upgrade failed: {reason}" emitted at the `execute_upgrade` level.

### 1.4 The caller in `upgrade_page.rs`

```rust
// src/ui/upgrade_page.rs, lines ~290-312
let (result_tx, result_rx) = async_channel::bounded::<bool>(1);

std::thread::spawn(move || {
    let success = upgrade::execute_upgrade(&distro2, &tx_clone);
    drop(tx_clone);
    let _ = result_tx.send_blocking(success);
});

drop(tx);

while let Ok(line) = rx.recv().await {
    log_ref2.append_line(&line);
}

let success = result_rx.recv().await.unwrap_or(false);
button_ref2.set_sensitive(true);

if success {
    crate::ui::reboot_dialog::show_reboot_dialog(&button_ref3);
}
```

- **On `false`:** the reboot dialog is not shown; the button becomes re-sensitive.
  No summary "Upgrade failed" message is appended to the log panel at the
  `upgrade_page.rs` level. The individual per-step failures are visible in
  the log only because `run_command_sync` streamed them via `tx`.
- **On `true`:** `show_reboot_dialog` is called.

### 1.5 The `result_rx` channel type

```rust
async_channel::bounded::<bool>(1)
```

This channel carries one value — the final success/failure flag —  
from the worker `std::thread::spawn` closure back to the GTK future.  
It is entirely separate from the log-message channel (`tx`/`rx`).

### 1.6 Other callers

A full-codebase search for `execute_upgrade` confirms there is **exactly one**
call site: `src/ui/upgrade_page.rs` line ~297.  
All other occurrences are in documentation files under `.github/docs/`.

---

## 2. Problem Definition

### 2.1 Anti-pattern: `bool` from a fallible function

`bool` is the appropriate return type when a function tests a condition
(e.g., `is_empty()`, `contains()`). For operations that can fail for
distinct, diagnosable reasons, `Result<T, E>` is the Rust idiom. The problems
with the current `bool` return:

1. **Lost context at the boundary.** When `execute_upgrade` returns `false` for
   an unsupported distro, the caller knows "it failed" but must independently
   reconstruct why (from the log channel stream, which it does not inspect for
   semantic meaning).
2. **Caller cannot distinguish failure modes.** The upgrade page cannot
   discriminate between "distro not supported" vs. "command exited non-zero"
   vs. "hostname validation rejected" without parsing log lines — a brittle
   coupling.
3. **No summary failure message exists.** The current flow has no single call
   that appends "Upgrade failed: [reason]" to the log panel. Individual
   step-level messages exist, but a user who reads only the last line of the
   log sees only the last streamed line, not a definitive failure summary.
4. **Blocks `?` propagation.** Returning `bool` prevents the use of `?` for
   clean error propagation within helper call stacks.
5. **`result_rx` channel carries semantically impoverished data.** A
   `bounded::<bool>(1)` channel that carries final outcome could carry
   `Result<(), String>` at the same cost, giving the GTK future access to
   the failure reason.

### 2.2 Why not a custom error enum?

A custom enum (e.g., `UpgradeError`) would be appropriate if:
- Callers match on specific variants to recover differently per variant
- The error type is part of a public library API

Neither applies here. `execute_upgrade` is an internal function; the GTK
future that receives the result always does the same thing (display the
message, re-enable the button). `String` is the correct lightweight choice:
idiomatic for internal error propagation where the message is human-readable
and callers do not need to match on specific variants.

---

## 3. Research Findings

### Source 1 — The Rust Book, Chapter 9: Error Handling
**URL:** https://doc.rust-lang.org/book/ch09-02-recoverable-errors-with-result.html

> "Rust doesn't have exceptions. Instead, it has the type `Result<T, E>` for
> recoverable errors."

The book strongly advocates `Result` for any fallible operation. It explicitly
distinguishes functions that "might work or might fail" from predicates that
return `bool`. The `execute_upgrade` function is a textbook case for `Result`:
it performs I/O, spawns processes, and can fail for multiple independent reasons.

**Idiom adopted:** `pub fn execute_upgrade(...) -> Result<(), String>`

---

### Source 2 — Rust by Example: Result
**URL:** https://doc.rust-lang.org/rust-by-example/error/result.html

Demonstrates that `Result<T, E>` eliminates the need for the caller to "know"
what went wrong out-of-band. The example of wrapping `std::io::Error` messages
into `String` via `.to_string()` is directly applicable to `execute_upgrade`'s
helper call chain.

**Key pattern:**
```rust
fn may_fail() -> Result<(), String> {
    // ...
    Err("descriptive reason".to_string())
}
```

---

### Source 3 — Rust API Design Guidelines: `Result` vs Fallible Predicates
**URL:** https://rust-lang.github.io/api-guidelines/documentation.html

The guidelines differentiate:
- **Predicates** → return `bool` (e.g., `Iterator::any`, `String::contains`)
- **Fallible operations** → return `Result<T, E>`

`execute_upgrade` performs side-effecting, multi-step system operations; it is
unambiguously a fallible operation, not a predicate. Returning `bool` violates
the API guidelines' recommended pattern.

---

### Source 4 — `String` vs Custom Error Enum — Rust Error Handling Best Practices
**URL:** https://doc.rust-lang.org/std/error/trait.Error.html  
**URL:** https://github.com/dtolnay/thiserror (reference)

For **public API boundaries** or **library code**, implementing `std::error::Error`
via `thiserror` is the recommended approach. For **internal application code**
where the error is always displayed as a string to the user and no variant
matching is needed by callers, `String` (or `Box<dyn Error>`) is acceptable.

`execute_upgrade` is internal to the `up` application. The GTK future in
`upgrade_page.rs` always surfaces the error as a log line. `String` is the
appropriate type — it avoids adding a dependency (`thiserror`) and introduces
no unnecessary abstraction overhead.

---

### Source 5 — The `?` Operator for Error Propagation
**URL:** https://doc.rust-lang.org/book/ch09-02-recoverable-errors-with-result.html#a-shortcut-for-propagating-errors-the--operator

The `?` operator is syntactic sugar for:
```rust
match expr {
    Ok(val) => val,
    Err(e) => return Err(e.into()),
}
```

Changing `execute_upgrade` (and helpers) to return `Result<(), String>` enables
the use of `?` in future helper functions that also return `Result<(), String>`.

**Note:** `run_command_sync` currently returns `bool`. This spec does **not**
change `run_command_sync` — that is a separate concern. Inside the helpers
the pattern will be:
```rust
if !crate::runner::run_command_sync("pkexec", &[...], tx) {
    return Err("Descriptive failure reason".to_string());
}
```

---

### Source 6 — GTK4-rs Async Channel Error Pattern
**URL:** https://gtk-rs.org/gtk4-rs/stable/latest/docs/gtk4/index.html  
**URL:** https://docs.rs/async-channel/latest/async_channel/

GTK4-rs applications commonly send operation results back to the GTK main loop
via `async_channel`. The channel type should match the semantic content of the
message. For a single-value result channel indicating the outcome of a
background operation, changing from `bounded::<bool>(1)` to
`bounded::<Result<(), String>>(1)` is the idiomatic upgrade:

```rust
// Before
let (result_tx, result_rx) = async_channel::bounded::<bool>(1);
// After
let (result_tx, result_rx) = async_channel::bounded::<Result<(), String>>(1);
```

The receiving GTK future then extracts the error string for display:
```rust
match result_rx.recv().await.unwrap_or(Err("Channel closed".to_string())) {
    Ok(()) => crate::ui::reboot_dialog::show_reboot_dialog(&button_ref3),
    Err(e) => log_ref2.append_line(&format!("Upgrade failed: {e}")),
}
```

---

## 4. Proposed Solution Architecture

### 4.1 Summary of Changes

1. Change `execute_upgrade` return type: `bool` → `Result<(), String>`
2. Change all four helper function return types: `bool` → `Result<(), String>`
3. In each helper, replace every `return false` with `return Err("...")` and
   the final implicit `bool` return with `Ok(())`
4. In `upgrade_page.rs`, change `result_tx`/`result_rx` channel type from
   `async_channel::bounded::<bool>(1)` to
   `async_channel::bounded::<Result<(), String>>(1)`
5. In `upgrade_page.rs`, replace the `if success { ... }` block with a
   `match` on the `Result<(), String>`

### 4.2 No Changes Needed

- `run_command_sync` in `src/runner.rs` — stays `-> bool`; this is a separate
  finding. The helpers continue to call it and translate `false` → `Err(...)`.
- `CheckMsg` enum in `upgrade_page.rs` — unchanged; it is for the check flow
- `async_channel::unbounded::<String>()` log channel — unchanged
- Any other file in `src/` — no other caller exists

---

## 5. Implementation Steps

### 5.1 `src/upgrade.rs` — `execute_upgrade`

**Before:**
```rust
pub fn execute_upgrade(distro: &DistroInfo, tx: &async_channel::Sender<String>) -> bool {
    let _ = tx.send_blocking(format!(
        "Starting upgrade for {} {}...",
        distro.name, distro.version
    ));

    match distro.id.as_str() {
        "ubuntu" | "debian" => upgrade_ubuntu(tx),
        "fedora" => upgrade_fedora(tx),
        "opensuse-leap" => upgrade_opensuse(tx),
        "nixos" => upgrade_nixos(tx),
        _ => {
            let _ = tx.send_blocking(format!(
                "Upgrade is not yet supported for '{}'. Supported: Ubuntu, Fedora, openSUSE Leap, NixOS.",
                distro.name
            ));
            false
        }
    }
}
```

**After:**
```rust
pub fn execute_upgrade(
    distro: &DistroInfo,
    tx: &async_channel::Sender<String>,
) -> Result<(), String> {
    let _ = tx.send_blocking(format!(
        "Starting upgrade for {} {}...",
        distro.name, distro.version
    ));

    match distro.id.as_str() {
        "ubuntu" | "debian" => upgrade_ubuntu(tx),
        "fedora" => upgrade_fedora(tx),
        "opensuse-leap" => upgrade_opensuse(tx),
        "nixos" => upgrade_nixos(tx),
        _ => {
            let msg = format!(
                "Upgrade is not yet supported for '{}'. Supported: Ubuntu, Debian, Fedora, openSUSE Leap, NixOS.",
                distro.name
            );
            let _ = tx.send_blocking(msg.clone());
            Err(msg)
        }
    }
}
```

> **Note:** The `_ =>` arm currently sends the message via `tx` and returns
> `false`. After the change it still sends via `tx` (for log continuity) and
> returns `Err(msg)`. This is intentional: the log panel shows it inline, and
> the caller appends a summary failure line using the same string.

---

### 5.2 `src/upgrade.rs` — `upgrade_ubuntu`

**Before:**
```rust
fn upgrade_ubuntu(tx: &async_channel::Sender<String>) -> bool {
    let _ = tx.send_blocking("Running: do-release-upgrade -f DistUpgradeViewNonInteractive".into());

    crate::runner::run_command_sync(
        "pkexec",
        &["do-release-upgrade", "-f", "DistUpgradeViewNonInteractive"],
        tx,
    )
}
```

**After:**
```rust
fn upgrade_ubuntu(tx: &async_channel::Sender<String>) -> Result<(), String> {
    let _ = tx.send_blocking("Running: do-release-upgrade -f DistUpgradeViewNonInteractive".into());

    if !crate::runner::run_command_sync(
        "pkexec",
        &["do-release-upgrade", "-f", "DistUpgradeViewNonInteractive"],
        tx,
    ) {
        return Err("Ubuntu/Debian upgrade command failed (see log for details)".to_string());
    }
    Ok(())
}
```

---

### 5.3 `src/upgrade.rs` — `upgrade_fedora`

**Before:** returns `bool` with four early `return false` paths.

**After:** returns `Result<(), String>`:

```rust
fn upgrade_fedora(tx: &async_channel::Sender<String>) -> Result<(), String> {
    // Step 1: Install upgrade plugin
    let _ = tx.send_blocking("Installing system-upgrade plugin...".into());
    if !crate::runner::run_command_sync(
        "pkexec",
        &["dnf", "install", "-y", "dnf-plugin-system-upgrade"],
        tx,
    ) {
        return Err("Failed to install dnf-plugin-system-upgrade (see log for details)".to_string());
    }

    // Step 2: Download upgrade packages (next version)
    let _ = tx.send_blocking("Downloading upgrade packages...".into());

    let next_version = match detect_next_fedora_version() {
        Some(v) => v,
        None => {
            let _ = tx.send_blocking(
                "Error: Could not detect current Fedora version. Aborting upgrade.".into(),
            );
            return Err("Could not detect current Fedora version to determine upgrade target".to_string());
        }
    };
    let ver_str = next_version.to_string();
    if !crate::runner::run_command_sync(
        "pkexec",
        &[
            "dnf",
            "system-upgrade",
            "download",
            "--releasever",
            &ver_str,
            "-y",
        ],
        tx,
    ) {
        return Err(format!(
            "Failed to download Fedora {} upgrade packages (see log for details)",
            next_version
        ));
    }

    // Step 3: Trigger reboot into upgrade
    let _ =
        tx.send_blocking("Download complete. The system will reboot to apply the upgrade.".into());
    if !crate::runner::run_command_sync("pkexec", &["dnf", "system-upgrade", "reboot"], tx) {
        return Err("Failed to trigger Fedora upgrade reboot (see log for details)".to_string());
    }
    Ok(())
}
```

---

### 5.4 `src/upgrade.rs` — `upgrade_opensuse`

**Before:**
```rust
fn upgrade_opensuse(tx: &async_channel::Sender<String>) -> bool {
    let _ = tx.send_blocking("Running zypper distribution upgrade...".into());
    crate::runner::run_command_sync("pkexec", &["zypper", "dup", "-y"], tx)
}
```

**After:**
```rust
fn upgrade_opensuse(tx: &async_channel::Sender<String>) -> Result<(), String> {
    let _ = tx.send_blocking("Running zypper distribution upgrade...".into());
    if !crate::runner::run_command_sync("pkexec", &["zypper", "dup", "-y"], tx) {
        return Err("openSUSE distribution upgrade command failed (see log for details)".to_string());
    }
    Ok(())
}
```

---

### 5.5 `src/upgrade.rs` — `upgrade_nixos`

**Before:** returns `bool` with four early `return false` paths across both
legacy-channel and flake paths.

**After:** returns `Result<(), String>`:

```rust
fn upgrade_nixos(tx: &async_channel::Sender<String>) -> Result<(), String> {
    const NIX_PATH_EXPORT: &str =
        "export PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin:$PATH";
    let config_type = detect_nixos_config_type();
    match config_type {
        NixOsConfigType::LegacyChannel => {
            let _ = tx.send_blocking("Detected: legacy channel-based NixOS config".into());
            let _ = tx.send_blocking("Updating NixOS channel...".into());
            let cmd = format!("{NIX_PATH_EXPORT} && nix-channel --update");
            if !crate::runner::run_command_sync("pkexec", &["sh", "-c", &cmd], tx) {
                return Err("Failed to update NixOS channel (see log for details)".to_string());
            }
            let _ = tx.send_blocking("Rebuilding NixOS (switch --upgrade)...".into());
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
        NixOsConfigType::Flake => {
            let _ = tx.send_blocking("Detected: flake-based NixOS config".into());
            let _ = tx.send_blocking("Updating flake inputs in /etc/nixos...".into());
            let cmd = format!("{NIX_PATH_EXPORT} && nix flake update --flake /etc/nixos");
            if !crate::runner::run_command_sync("pkexec", &["sh", "-c", &cmd], tx) {
                return Err(
                    "Failed to update flake inputs in /etc/nixos (see log for details)".to_string(),
                );
            }
            let raw_hostname = detect_hostname();
            let hostname = match validate_hostname(&raw_hostname) {
                Ok(h) => h,
                Err(e) => {
                    let msg = format!("Upgrade aborted: {e}");
                    let _ = tx.send_blocking(msg.clone());
                    return Err(msg);
                }
            };
            let flake_target = format!("/etc/nixos#{}", hostname);
            let _ = tx.send_blocking(format!("Rebuilding NixOS configuration: {flake_target}"));
            if !crate::runner::run_command_sync(
                "pkexec",
                &["nixos-rebuild", "switch", "--flake", &flake_target],
                tx,
            ) {
                return Err(format!(
                    "Failed to rebuild NixOS flake configuration '{}' (see log for details)",
                    flake_target
                ));
            }
            Ok(())
        }
    }
}
```

---

### 5.6 `src/ui/upgrade_page.rs` — caller update

**Before (channel declaration and worker thread):**
```rust
let (result_tx, result_rx) = async_channel::bounded::<bool>(1);

std::thread::spawn(move || {
    let success = upgrade::execute_upgrade(&distro2, &tx_clone);
    drop(tx_clone);
    let _ = result_tx.send_blocking(success);
});
```

**After:**
```rust
let (result_tx, result_rx) = async_channel::bounded::<Result<(), String>>(1);

std::thread::spawn(move || {
    let outcome = upgrade::execute_upgrade(&distro2, &tx_clone);
    drop(tx_clone);
    let _ = result_tx.send_blocking(outcome);
});
```

**Before (GTK future — result consumption):**
```rust
let success = result_rx.recv().await.unwrap_or(false);
button_ref2.set_sensitive(true);

if success {
    crate::ui::reboot_dialog::show_reboot_dialog(&button_ref3);
}
```

**After:**
```rust
let outcome = result_rx
    .recv()
    .await
    .unwrap_or_else(|_| Err("Upgrade result channel closed unexpectedly".to_string()));
button_ref2.set_sensitive(true);

match outcome {
    Ok(()) => {
        crate::ui::reboot_dialog::show_reboot_dialog(&button_ref3);
    }
    Err(e) => {
        log_ref2.append_line(&format!("Upgrade failed: {e}"));
    }
}
```

> **Why `unwrap_or_else`?** The original `unwrap_or(false)` silently treated a
> disconnected channel as failure. The updated form returns an `Err` with a
> diagnostic message so the log panel shows something meaningful if the channel
> closes unexpectedly.

---

## 6. Error Messages — Mapping Table

| Path # | Trigger | `Err(String)` message |
|--------|---------|----------------------|
| 1 | Unsupported distro | `"Upgrade is not yet supported for '{name}'. Supported: Ubuntu, Debian, Fedora, openSUSE Leap, NixOS."` |
| 2 | Ubuntu: `do-release-upgrade` fails | `"Ubuntu/Debian upgrade command failed (see log for details)"` |
| 3 | Fedora step 1: plugin install fails | `"Failed to install dnf-plugin-system-upgrade (see log for details)"` |
| 4 | Fedora step 2a: version detection fails | `"Could not detect current Fedora version to determine upgrade target"` |
| 5 | Fedora step 2b: package download fails | `"Failed to download Fedora {N} upgrade packages (see log for details)"` |
| 6 | Fedora step 3: reboot trigger fails | `"Failed to trigger Fedora upgrade reboot (see log for details)"` |
| 7 | openSUSE: `zypper dup -y` fails | `"openSUSE distribution upgrade command failed (see log for details)"` |
| 8 | NixOS legacy step 1: channel update fails | `"Failed to update NixOS channel (see log for details)"` |
| 9 | NixOS legacy step 2: rebuild fails | `"Failed to rebuild NixOS with --upgrade (see log for details)"` |
| 10 | NixOS flake step 1: flake update fails | `"Failed to update flake inputs in /etc/nixos (see log for details)"` |
| 11 | NixOS flake step 2: hostname invalid | `"Upgrade aborted: {validate_hostname error}"` |
| 12 | NixOS flake step 3: rebuild fails | `"Failed to rebuild NixOS flake configuration '/etc/nixos#{host}' (see log for details)"` |

The phrase "(see log for details)" is included for paths triggered by
`run_command_sync` failures, because `run_command_sync` already streams the
specific exit code / stderr lines to the log panel via `tx`. The Err string is
a summary; the log contains the full context.

---

## 7. Unit Tests

### 7.1 Testable paths

Most paths in `execute_upgrade` cannot be unit-tested without mocking because
they spawn real system processes (`pkexec`, `do-release-upgrade`, etc.).

The following paths **can** be tested in isolation:

| Path | Test approach |
|------|--------------|
| Path 1 — unsupported distro | Construct a `DistroInfo` with `id = "arch"` and pass it to `execute_upgrade`. The function early-returns before calling any external command. Assert `Err(e)` where `e` contains `"not yet supported"`. Requires a test `async_channel`. |
| Path 4 — Fedora version detection | `detect_next_fedora_version()` is already a free function; its `None` path is testable by not having `rpm` available (or by running the test in an environment without it). Not directly injectable without mocking. |
| Path 11 — hostname validation | `validate_hostname()` is already pure and fully tested; no new tests needed here since the guard is already covered by the existing 8 hostname tests. |

### 7.2 Test for path 1 (unsupported distro)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_upgrade_unsupported_distro_returns_err() {
        let distro = DistroInfo {
            id: "arch".to_string(),
            name: "Arch Linux".to_string(),
            version: "2026.01.01".to_string(),
            version_id: "2026".to_string(),
            upgrade_supported: false,
        };
        let (tx, _rx) = async_channel::unbounded::<String>();
        let result = execute_upgrade(&distro, &tx);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("not yet supported"),
            "unexpected message: {msg}"
        );
    }
}
```

### 7.3 Note on mocking

`run_command_sync` is not abstracted behind a trait; paths 2–12 cannot be unit-
tested without either real system commands or introducing a mock abstraction for
the runner. This is out of scope for this change. The existing preflight
validates a real `cargo build` + `cargo test` pass, which is sufficient.

---

## 8. Risks and Mitigations

| Risk | Severity | Mitigation |
|------|----------|-----------|
| Other callers of `execute_upgrade` break at compile time | Low | Confirmed: exactly one caller (`upgrade_page.rs` line ~297). Changing the channel type from `bounded::<bool>` to `bounded::<Result<(), String>>` is a compile-time-safe change. |
| `unwrap_or(false)` → `unwrap_or_else(...)` semantics change | Low | Old code defaulted silently to "failure, no reboot dialog". New code defaults to `Err(...)` with a diagnostic message shown in the log. This is strictly better. |
| `NixOsConfigType::Flake` path: "Upgrade aborted: {e}" is sent twice | None | In the flake hostname-invalid path (path 11), `tx.send_blocking(msg.clone())` sends the message once *before* `return Err(msg)`. The caller in `upgrade_page.rs` then appends `"Upgrade failed: {e}"`. The two messages are distinct: the first is the raw abort reason, the second is a summary line. This is acceptable; it matches the existing pattern where `run_command_sync` already streams per-step lines and the caller adds a summary. |
| Log panel receives "Upgrade failed: X" AND inline "X" (duplication) | Low | For path 11, the log will contain "Upgrade aborted: invalid hostname: ..." (sent by `upgrade_nixos`) AND "Upgrade failed: Upgrade aborted: invalid hostname: ..." (appended by `upgrade_page.rs`). For path 1, both are the same string. This is minor redundancy; acceptable because it makes the final "Upgrade failed: ..." line easy to find at the bottom of the log. |
| `helper_fn` returning `Ok(())` on success introduces `Result` wrapping overhead | None | `Result<(), String>` has zero overhead on the `Ok(())` path in a non-hot-path function like upgrade helpers. |
| Forgetting to change one of the four helper function signatures | Low | Compiler will catch it: `execute_upgrade`'s match arms call the helpers and their return values are directly propagated. A `bool`-returning helper would produce a type mismatch compile error. |

---

## 9. Files Modified

| File | Change |
|------|--------|
| `src/upgrade.rs` | Change `execute_upgrade`, `upgrade_ubuntu`, `upgrade_fedora`, `upgrade_opensuse`, `upgrade_nixos` return types and bodies |
| `src/ui/upgrade_page.rs` | Change `result_tx`/`result_rx` channel type; change result consumption block |

No other files are modified.

---

## 10. Dependencies

No new dependencies. No changes to `Cargo.toml`, `meson.build`, or `flake.nix`.

---

## 11. Acceptance Criteria

- [ ] `cargo build` succeeds with zero errors
- [ ] `cargo clippy -- -D warnings` produces zero warnings
- [ ] `cargo fmt --check` passes
- [ ] `cargo test` passes (all 8 existing tests + any new test for path 1)
- [ ] `execute_upgrade` signature is `pub fn execute_upgrade(distro: &DistroInfo, tx: &async_channel::Sender<String>) -> Result<(), String>`
- [ ] All four helper functions return `Result<(), String>`
- [ ] `result_tx`/`result_rx` in `upgrade_page.rs` is typed `async_channel::bounded::<Result<(), String>>(1)`
- [ ] On `Ok(())`, reboot dialog is shown (behaviour unchanged)
- [ ] On `Err(e)`, `"Upgrade failed: {e}"` is appended to the log panel
- [ ] No other callers of `execute_upgrade` exist outside `upgrade_page.rs`
