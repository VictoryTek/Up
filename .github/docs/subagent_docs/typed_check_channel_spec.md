# Typed Check Channel — Implementation Specification

**Feature**: Replace the `"__RESULTS__:"` string-sentinel channel protocol with a typed `CheckMsg` enum channel in `src/ui/upgrade_page.rs`.  
**Date**: 2026-03-18  
**Status**: Ready for implementation

---

## 1. Current State Analysis

### 1.1 Channel Element Type and Declaration

The prerequisite-check worker uses a single `async_channel::unbounded::<String>()` channel declared inside the `glib::spawn_future_local` async block that is created by the `check_button.connect_clicked` handler.

File: `src/ui/upgrade_page.rs`, lines 241–242:

```rust
let (tx, rx) = async_channel::unbounded::<String>();
let tx_clone = tx.clone();
```

- `tx` — `async_channel::Sender<String>`, owned by the async task; dropped immediately after spawn to close the channel when the worker finishes
- `tx_clone` — clone of `tx` moved into the `std::thread::spawn` worker closure
- `rx` — `async_channel::Receiver<String>`, polled by the GTK `spawn_future_local` async loop

### 1.2 All Send Sites

**Send sites inside `upgrade::run_prerequisite_checks` (src/upgrade.rs lines 127, 136, 141) — log lines:**

```rust
let _ = tx.send_blocking("Checking if all packages are up to date...".into());
let _ = tx.send_blocking("Checking available disk space...".into());
let _ = tx.send_blocking("Backup check...".into());
```

These three are plain diagnostic log lines forwarded to the `LogPanel`.

**Send site inside the `std::thread::spawn` worker closure (src/ui/upgrade_page.rs lines 247–250) — the sentinel message:**

```rust
let json = serde_json::to_string(&results).unwrap_or_default();
let _ = tx_clone.send_blocking(format!("__RESULTS__:{json}"));
drop(tx_clone);
```

This is the only send that carries structured data.  The `results` value is a `Vec<upgrade::CheckResult>` returned synchronously by `run_prerequisite_checks`.

### 1.3 The Receive Loop

`src/ui/upgrade_page.rs` lines 254–276:

```rust
let mut all_passed = true;
while let Ok(msg) = rx.recv().await {
    if let Some(json) = msg.strip_prefix("__RESULTS__:") {
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
        // ← serde_json failure: results silently dropped, `all_passed` stays `true`
    } else {
        log_ref.append_line(&msg);
    }
}
```

### 1.4 The Second, Separate Channel (`result_tx` / `result_rx`)

Within the **upgrade button** callback (`upgrade_button.connect_clicked`) there is a second distinct channel:

```rust
let (result_tx, result_rx) = async_channel::bounded::<bool>(1);
```

This channel carries a single `bool` success value from `upgrade::execute_upgrade` back to the GTK future. It is **entirely separate** from the prerequisite-check channel and is **not in scope** for this refactor. The upgrade button callback also has its own `async_channel::unbounded::<String>()` for `execute_upgrade` log lines; that channel is also separate and unchanged.

### 1.5 `serde` / `serde_json` Dependency Status

`Cargo.toml` contains:

```toml
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

Both are already present. `serde_json` is also used independently in `src/backends/nix.rs` (parsing flake lock files), so the `serde_json` entry in `Cargo.toml` **must be retained** after this refactor.

### 1.6 `upgrade::CheckResult` Derive Status

`src/upgrade.rs` lines 14–18:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub name: String,
    pub passed: bool,
    pub message: String,
}
```

`Serialize` and `Deserialize` are **already derived**. After this refactor the JSON round-trip is removed from `upgrade_page.rs`, but these derives may be retained for future use (e.g., D-Bus serialisation, config persistence) without penalty. They are not removed as part of this change.

---

## 2. Problem Definition

### 2.1 Silent-Drop on Serialisation Failure

```rust
let json = serde_json::to_string(&results).unwrap_or_default();
let _ = tx_clone.send_blocking(format!("__RESULTS__:{json}"));
```

If `serde_json::to_string` returns an `Err`, `unwrap_or_default()` produces an empty string. The message sent becomes `"__RESULTS__:"`. On the receive side:

```rust
if let Ok(results) = serde_json::from_str::<Vec<upgrade::CheckResult>>(json) {
```

Deserialising an empty JSON document fails silently — the `if let Ok` arm is skipped, the check-result rows remain at "Checking...", `all_passed` stays `true`, and the **Start Upgrade** button may be enabled even though no checks actually completed.

### 2.2 Silent-Drop on Deserialisation Failure

If the serialised JSON is valid but does not match `Vec<upgrade::CheckResult>` (e.g., a future schema change, or a log line that happens to start with `"__RESULTS__:"`), the `if let Ok` arm is skipped with no user feedback.

### 2.3 Magic-Prefix Coupling

The string `"__RESULTS__:"` is an informal protocol contract between two parts of the same function. It is:

- Undocumented (not a type, not a constant)
- Vulnerable to accidental collision — any `upgrade.rs` log line beginning with `"__RESULTS__:"` would be misinterpreted as structured data
- A maintenance hazard — anyone adding a log line must know not to use this prefix

### 2.4 Why Typed Channels Are the Idiomatic Solution

Rust's type system makes this class of bug impossible: replacing `Sender<String>` with `Sender<CheckMsg>` forces every variant to be handled in the match arm at compile time. No sentinel string, no JSON round-trip, no silent drops.

---

## 3. Proposed Solution Architecture

### 3.1 New `CheckMsg` Enum

Define the following private enum at the top of `src/ui/upgrade_page.rs` (inside the module, not `pub`):

```rust
enum CheckMsg {
    Log(String),
    Results(Vec<upgrade::CheckResult>),
    Error(String),
}
```

- `Log` — a plain diagnostic line to be forwarded to `LogPanel`
- `Results` — the structured check results returned by `run_prerequisite_checks`; no serialisation needed
- `Error` — conveys an unexpected worker-thread error to the UI for display

### 3.2 `upgrade.rs` Is Left Completely Unchanged

`run_prerequisite_checks` currently has the signature:

```rust
pub fn run_prerequisite_checks(
    distro: &DistroInfo,
    tx: &async_channel::Sender<String>,
) -> Vec<CheckResult>
```

Changing this signature to accept `Sender<CheckMsg>` would require `upgrade.rs` to import a private type from `ui::upgrade_page`, creating an inverted dependency (a higher-level module importing from a UI sub-module). This is incorrect architecture and is **not done**.

Instead, the worker thread in `upgrade_page.rs` uses a **bridge channel**:

1. Creates a temporary `async_channel::unbounded::<String>()` bridge pair
2. Passes the bridge sender to `run_prerequisite_checks` (unchanged call)
3. After `run_prerequisite_checks` returns, drains the bridge channel synchronously via `recv_blocking()`, re-wrapping each message as `CheckMsg::Log` and forwarding to the outer `CheckMsg` sender
4. Sends `CheckMsg::Results(results)` directly — no JSON serialisation

Because `run_prerequisite_checks` is synchronous and calls `send_blocking()` on an unbounded channel (which never blocks), all log messages are buffered in the bridge by the time the function returns. The drain is therefore immediate. The three diagnostic log lines ("Checking if all packages are up to date...", etc.) appear in the `LogPanel` in the same logical order as today. Since the three checks complete in well under one second, the negligible batching difference is imperceptible to the user.

### 3.3 Channel Scope Summary

| Channel variable | Type | Owner | Used for |
|---|---|---|---|
| `(check_tx, check_rx)` | `async_channel::unbounded::<CheckMsg>()` | check-button async task | prerequisite check results and log lines — **this refactor** |
| `(bridge_tx, bridge_rx)` | `async_channel::unbounded::<String>()` | worker `std::thread` only | forward `run_prerequisite_checks` log strings into `check_tx` |
| `(tx, rx)` in upgrade callback | `async_channel::unbounded::<String>()` | upgrade-button async task | `execute_upgrade` log lines — **unchanged** |
| `(result_tx, result_rx)` | `async_channel::bounded::<bool>(1)` | upgrade-button async task | `execute_upgrade` success flag — **unchanged** |

### 3.4 New Receive Loop

Replace the `strip_prefix` branch with a direct `match`:

```rust
while let Ok(msg) = check_rx.recv().await {
    match msg {
        CheckMsg::Log(line) => {
            log_ref.append_line(&line);
        }
        CheckMsg::Results(results) => {
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
        CheckMsg::Error(e) => {
            log_ref.append_line(&format!("[error] {e}"));
            all_passed = false;
        }
    }
}
```

---

## 4. Implementation Steps

### Step 1 — Add `CheckMsg` enum

Insert the following immediately before the `pub struct UpgradePage;` line in `src/ui/upgrade_page.rs`:

```rust
/// Typed messages sent from the prerequisite-check worker thread to the GTK UI loop.
/// Private to this module — `upgrade.rs` is not affected.
enum CheckMsg {
    Log(String),
    Results(Vec<upgrade::CheckResult>),
    Error(String),
}
```

### Step 2 — Replace the channel declaration

Inside the `check_button.connect_clicked` async task, **replace**:

```rust
let (tx, rx) = async_channel::unbounded::<String>();

let tx_clone = tx.clone();
let distro_thread = distro.clone();
```

**with**:

```rust
let (check_tx, check_rx) = async_channel::unbounded::<CheckMsg>();

let check_tx_clone = check_tx.clone();
let distro_thread = distro.clone();
```

### Step 3 — Replace the `std::thread::spawn` worker closure

**Replace** the entire `std::thread::spawn` block:

```rust
std::thread::spawn(move || {
    let results = upgrade::run_prerequisite_checks(&distro_thread, &tx_clone);
    // Send results as serialized
    let json = serde_json::to_string(&results).unwrap_or_default();
    let _ = tx_clone.send_blocking(format!("__RESULTS__:{json}"));
    drop(tx_clone);
});
```

**with**:

```rust
std::thread::spawn(move || {
    // Bridge channel: run_prerequisite_checks keeps its existing &Sender<String> signature.
    // Messages are buffered here and forwarded as CheckMsg::Log after the call returns.
    let (bridge_tx, bridge_rx) = async_channel::unbounded::<String>();
    let results = upgrade::run_prerequisite_checks(&distro_thread, &bridge_tx);
    drop(bridge_tx); // close bridge; all log messages are now in bridge_rx

    // Forward log lines — no JSON, no magic prefix
    while let Ok(msg) = bridge_rx.recv_blocking() {
        let _ = check_tx_clone.send_blocking(CheckMsg::Log(msg));
    }

    // Send results directly — no serialisation needed
    let _ = check_tx_clone.send_blocking(CheckMsg::Results(results));
    drop(check_tx_clone);
});
```

### Step 4 — Update the `drop(tx)` line

**Replace**:

```rust
drop(tx);
```

**with**:

```rust
drop(check_tx);
```

### Step 5 — Replace the receive loop

**Replace** the entire `while let Ok(msg) = rx.recv().await { ... }` block:

```rust
let mut all_passed = true;
while let Ok(msg) = rx.recv().await {
    if let Some(json) = msg.strip_prefix("__RESULTS__:") {
        if let Ok(results) = serde_json::from_str::<Vec<upgrade::CheckResult>>(json)
        {
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
    } else {
        log_ref.append_line(&msg);
    }
}
```

**with**:

```rust
let mut all_passed = true;
while let Ok(msg) = check_rx.recv().await {
    match msg {
        CheckMsg::Log(line) => {
            log_ref.append_line(&line);
        }
        CheckMsg::Results(results) => {
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
        CheckMsg::Error(e) => {
            log_ref.append_line(&format!("[error] {e}"));
            all_passed = false;
        }
    }
}
```

### Step 6 — Verify no other usages of `serde_json` remain in `upgrade_page.rs`

After Steps 2–5, the two `serde_json::` call sites at lines 248 and 258 will be gone. Confirm with `grep serde_json src/ui/upgrade_page.rs` — expected: no output. No explicit `use serde_json` top-level import existed in this file (the code used fully-qualified paths), so no `use` statement needs removing.

`serde_json` remains in `Cargo.toml` — it is still used in `src/backends/nix.rs` for flake lock parsing. **Do not remove it from `Cargo.toml`.**

### Step 7 — Build and lint

```sh
cargo build
cargo clippy -- -D warnings
cargo fmt --check
cargo test
```

All must pass with zero errors and zero warnings before the change is considered complete.

---

## 5. Dependencies

| Dependency | Status | Action |
|---|---|---|
| `async-channel = "2"` | Already in Cargo.toml | No change |
| `serde = { version = "1", features = ["derive"] }` | Already in Cargo.toml | No change |
| `serde_json = "1"` | Already in Cargo.toml | **Retain** (still used in `backends/nix.rs`) |

No new Cargo dependencies are added. The JSON round-trip is eliminated entirely from the prerequisite-check code path. `serde_json` is no longer referenced in `upgrade_page.rs` after the change.

---

## 6. Affected Files

| File | Change | Notes |
|---|---|---|
| `src/ui/upgrade_page.rs` | **Primary change** | Add `CheckMsg` enum; replace channel type, spawn closure, drop, and receive loop |
| `src/upgrade.rs` | **No change** | `run_prerequisite_checks` signature and body are untouched; bridge channel pattern preserves the existing `&Sender<String>` interface |
| `Cargo.toml` | **No change** | `serde_json` retained for `backends/nix.rs` |

---

## 7. Risks and Mitigations

### Risk 1 — `CheckMsg` must be `Send`

The `std::thread::spawn` worker closure moves `check_tx_clone: async_channel::Sender<CheckMsg>` across a thread boundary, which requires `CheckMsg: Send`.

**Analysis**: `CheckMsg` contains only `String` and `Vec<upgrade::CheckResult>`. `String` is `Send`. `CheckResult` is `#[derive(Debug, Clone, Serialize, Deserialize)]` and contains only `String` and `bool` fields — both `Send`. Therefore `CheckMsg: Send` holds automatically. The `bridge_rx: async_channel::Receiver<String>` is also `Send`. No manual `unsafe impl` is needed.

### Risk 2 — Confusing the `CheckMsg` channel with the `bool` result channel

The upgrade-button callback declares a separate `async_channel::bounded::<bool>(1)` channel (`result_tx` / `result_rx`). This channel is physically in a different closure — a different `glib::spawn_future_local` task spawned from `upgrade_button.connect_clicked`. It does not share any variables with the `check_button.connect_clicked` closure. Renaming the prerequisite-check channel to `check_tx` / `check_rx` (from `tx` / `rx`) makes the distinction explicit and eliminates any risk of confusion within the check-button closure.

### Risk 3 — `serde_json` import becoming stale in `upgrade_page.rs`

After the refactor, both `serde_json::to_string` and `serde_json::from_str` calls are removed from `upgrade_page.rs`. Because the code used fully-qualified `serde_json::` paths rather than a `use serde_json::...` import statement, no top-level `use` item needs removing. Cargo will not emit an "unused import" warning. However, if `cargo clippy` is run in pedantic mode, it may flag the crate as unused in this file — this is benign since the crate is still used in `backends/nix.rs`.

### Risk 4 — Log message ordering change

The bridge approach drains the bridge channel **after** `run_prerequisite_checks` returns, not concurrently with it. In the current code the log messages stream to the GTK loop in real time as each check completes. After the change, all three log messages are batched and appear together once the final check finishes.

**Mitigation**: The three checks complete in under one second in all tested environments. The batching is imperceptible to users. If real-time streaming is required in the future, the bridge drain can be moved to a dedicated forwarding `std::thread` (joined before sending `CheckMsg::Results`) without any further interface changes.

### Risk 5 — `CheckMsg::Error` variant is currently unreachable

`run_prerequisite_checks` does not return `Err` — all failure states are encoded as `CheckResult { passed: false, ... }`. The `CheckMsg::Error` variant has no current producer. Clippy will **not** warn about an unreachable match arm because the `match` patterns are exhaustive, not dead-code. The variant is included for forward compatibility (e.g., if a future refactor adds a fallible fast-path before calling `run_prerequisite_checks`).

---

## 8. Non-Goals

- Changing `upgrade::run_prerequisite_checks` or any other function in `upgrade.rs`
- Applying the typed-channel pattern to the upgrade-execution flow (`execute_upgrade`, the `bool` result channel)
- Removing `#[derive(Serialize, Deserialize)]` from `CheckResult`
- Removing `serde_json` from `Cargo.toml`
- Adding real-time log streaming (acceptable future enhancement, not required now)
