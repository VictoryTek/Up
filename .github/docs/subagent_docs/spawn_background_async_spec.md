# Specification: `spawn_background_async` Helper Extraction

**Feature:** `spawn_background_async`
**Author:** Research subagent (Phase 1)
**Date:** 2026-03-19

---

## 1. Current State Analysis

### 1.1 Actual occurrence count

A global grep for `tokio::runtime::Builder::new_current_thread` across `src/**/*.rs`
returns **exactly 2 matches**, both in `src/ui/window.rs`:

| # | File | Line (approx.) | Label |
|---|------|----------------|-------|
| A | `src/ui/window.rs` | ~157 | "Update All" button |
| B | `src/ui/window.rs` | ~260 | Per-backend availability check |

> **Important discrepancy from the task prompt.**
> The task stated "four near-identical copies" including `upgrade_page.rs`.
> Inspection reveals that `upgrade_page.rs` contains **three** `std::thread::spawn`
> calls (lines ~177, ~255, ~355), but **none** of them create a Tokio runtime.
> All three run purely synchronous code (`upgrade::check_upgrade_available`,
> `upgrade::run_prerequisite_checks`, `upgrade::execute_upgrade`) and communicate
> via `async_channel::Sender::send_blocking`. They are a distinct pattern and are
> **outside the scope of this refactoring**.

---

### 1.2 Occurrence A — "Update All" (`window.rs` ~L147–202)

**Context:** Inside `glib::spawn_future_local(async move { ... })` triggered by the
"Update All" button click.

**Full pattern:**

```rust
std::thread::spawn(move || {
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => {
            rt.block_on(async {
                for backend in &backends_thread {
                    let kind = backend.kind();
                    let runner = CommandRunner::new(tx_thread.clone(), kind);
                    let result = backend.run_update(&runner).await;
                    let _ = result_tx_thread.send((kind, result)).await;
                }
            });
        }
        Err(e) => {
            // Send an error result for every backend so the UI exits its recv loop
            for backend in &backends_thread {
                let kind = backend.kind();
                let _ = result_tx_thread.send_blocking((
                    kind,
                    crate::backends::UpdateResult::Error(format!(
                        "Runtime error: {e}"
                    )),
                ));
            }
        }
    }

    drop(tx_thread);       // redundant — drops at end of closure anyway
    drop(result_tx_thread); // redundant — drops at end of closure anyway
});
```

**Captured variables:**

| Variable | Type | Source |
|----------|------|--------|
| `backends_thread` | `Vec<Box<dyn Backend>>` | Cloned from `detected_clone` before spawn |
| `tx_thread` | `async_channel::Sender<(BackendKind, String)>` | Cloned from `tx` |
| `result_tx_thread` | `async_channel::Sender<(BackendKind, UpdateResult)>` | Cloned from `result_tx` |

**Async block body:** Iterates all backends sequentially. For each backend, creates a
`CommandRunner`, calls `backend.run_update(&runner).await`, sends the `(BackendKind,
UpdateResult)` tuple to `result_tx_thread`. Log output streams through `tx_thread`.

**Runtime failure error path:** Sends `UpdateResult::Error("Runtime error: {e}")` for
**every** backend via `send_blocking`, ensuring the UI's `result_rx.recv()` loop drains
and exits cleanly. Without this, the UI would show all rows stuck in the "running"
spinner animation and the status label would incorrectly read "Update complete."

---

### 1.3 Occurrence B — Per-backend availability check (`window.rs` ~L258–285)

**Context:** Inside the `run_checks: Rc<dyn Fn()>` closure, called once per backend
inside `glib::spawn_future_local(async move { ... })`.

**Full pattern:**

```rust
std::thread::spawn(move || {
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => {
            rt.block_on(async {
                let result = backend_clone.count_available().await;
                let _ = tx.send(result).await;
            });
        }
        Err(e) => {
            let _ = tx.send_blocking(Err(format!("Runtime error: {e}")));
        }
    }
});
```

**Captured variables:**

| Variable | Type | Source |
|----------|------|--------|
| `backend_clone` | `Box<dyn Backend>` | Cloned per-iteration from `detected` |
| `tx` | `async_channel::Sender<Result<usize, String>>` | Bounded channel (cap 1) |

**Async block body:** Calls `backend_clone.count_available().await` and sends the
`Result<usize, String>` on `tx`.

**Runtime failure error path:** Sends `Err("Runtime error: {e}")` via `send_blocking`,
causing the UI to call `row.set_status_unknown(msg)`. Without this, `rx.recv().await`
returns `Err` (channel closed with no value); `if let Ok(result) = rx.recv().await`
silently does not execute, leaving the row in the "Checking…" state indefinitely.

---

### 1.4 What is identical across both occurrences

- `std::thread::spawn(move || { ... })`
- `tokio::runtime::Builder::new_current_thread().enable_all().build()`
- `match ... { Ok(rt) => { rt.block_on(async { ... }); } Err(e) => { ... } }`
- Error path structure: send a "Runtime error: {e}" message through a channel using
  `send_blocking`

### 1.5 What varies between occurrences

| Dimension | Occurrence A | Occurrence B |
|-----------|-------------|-------------|
| Async block body | Multi-backend update loop | Single `count_available()` call |
| Captured types | 3 variables including `Vec<Box<dyn Backend>>` | 2 variables |
| Error path complexity | One error per backend, multi-item loop | Single `send_blocking(Err(...))` |
| Post-match explicit drops | Yes (redundant) | No |

---

## 2. Problem Definition

1. **DRY violation:** The 8-line runtime setup/teardown pattern (`std::thread::spawn` +
   `Builder::new_current_thread()` + `enable_all()` + `build()` + `match` + `block_on`)
   is duplicated in 2 call sites today, with no abstraction between them.

2. **Maintenance burden:** Every future async background task in the UI (more backends,
   more operations) would copy this boilerplate again, drifting independently.

3. **Redundant explicit drops:** Occurrence A contains `drop(tx_thread)` and
   `drop(result_tx_thread)` after the match. These are semantically redundant — the
   variables are already dropped when the closure returns — and would be silently
   carried into new copies.

4. **Inconsistent error handling for runtime build failures:** Neither occurrence uses
   structured error types or a consistent strategy. The pattern is "send some string
   over the nearest available channel," which differs in detail between occurrences.
   Runtime build failure is effectively unreachable in production (requires system-level
   resource exhaustion), making elaborate error paths disproportionate to the risk.

---

## 3. Proposed Solution Architecture

### 3.1 Chosen approach: Option A (simple `eprintln!` on build failure)

**Recommendation: Option A.** The rationale:

- `tokio::runtime::Builder::new_current_thread().enable_all().build()` failing in a
  healthy Linux GTK application is essentially unreachable. It requires, e.g.,
  OS-level `pthread_create` failure due to resource exhaustion. Neither call site has
  a tested or verified recovery path for this today.
- The existing error paths are already inconsistent (Occurrence A loops; Occurrence B
  sends a single message). Neither is a deliberate, tested error-recovery strategy.
- Option B (adding an `on_error: impl FnOnce(io::Error) + Send + 'static` parameter)
  requires each call site to pre-clone channel senders a second time and pass a second
  closure, significantly complicating the API for a case that never fires in practice.
- `eprintln!` on a runtime build failure is both visible to developers and appropriate
  for a truly exceptional, unrecoverable situation.

**Documented observable behavior change (runtime build failure edge case):**

| Event | Current | After refactor |
|-------|---------|----------------|
| Occurrence A: runtime fails | Rows show "Runtime error: …" + status "Update completed with errors." | Rows stuck in running state + status "Update complete." (silent UI inconsistency) |
| Occurrence B: runtime fails | Row shows `set_status_unknown("Runtime error: …")` | Row stays in "Checking…" state |

Both changes affect only the unreachable `runtime::Builder::build()` failure path.
If this concerns reviewers, Option B is straightforward to add later.

### 3.2 Helper signature

```rust
use std::future::Future;

/// Spawns a new OS thread, creates a single-threaded Tokio runtime on it,
/// and drives the future returned by `f` to completion via `block_on`.
///
/// If the Tokio runtime fails to build (resource exhaustion — effectively
/// unreachable in normal operation), an error is printed to stderr and the
/// thread exits without producing any output on caller channels.
pub(crate) fn spawn_background_async<F, Fut>(f: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
{
    std::thread::spawn(move || {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => {
                rt.block_on(f());
            }
            Err(e) => {
                eprintln!("Failed to build Tokio runtime: {e}");
            }
        }
    });
}
```

**Why `Fut: Future<Output = ()>` without `'static` or `Send`:**

- `tokio::runtime::Runtime::block_on` signature is `pub fn block_on<F: Future>(&self, future: F) -> F::Output` — no `'static`, no `Send`.
- `Fut` is created INSIDE the spawned thread (by calling `f()`). It never crosses a thread boundary. Therefore `Fut: Send` is not required.
- `Fut: 'static` is also not required because `block_on` does not require it, and the future is created and fully consumed within the same thread.
- `F: Send + 'static` IS required because `F` itself crosses the thread boundary inside `std::thread::spawn`.

**Lifetime safety at call sites:**

Both `async move { ... }` blocks in the converted call sites capture data that is fully
`'static` (`Vec<Box<dyn Backend>>` — defaulting to `Box<dyn Backend + 'static>`;
`async_channel::Sender<T>` — always `'static`). Internal borrows within the async state
machine (e.g., `for backend in &backends_thread` where `backends_thread` is owned by
the state machine) are self-referential and are correctly handled by Rust's async
desugaring via `Pin`. No non-`'static` external data is captured.

> **Note to implementer:** The one subtle point is `backend.run_update(&runner)`, which
> returns `Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>>` where `'a` is the
> lifetime of `&backend` and `&runner`. Both `backend` and `runner` are owned/borrowed
> from within the async state machine itself, making this a self-referential borrow
> handled by `Pin`. If the compiler rejects this (unlikely, but possible depending on
> exact Rust version and inference), the mitigation is to call `run_update` through a
> local binding that explicitly moves rather than borrows — or to accept that `Fut`
> cannot be `'static` and use `rt.block_on(f())` directly (which does not require it).
> Either way, the non-`'static` signature above is the correct starting point.

### 3.3 Placement

**`src/ui/mod.rs`** — accessible to `window.rs` and `upgrade_page.rs` as
`super::spawn_background_async(...)`.

Rationale:
- Only the `window.rs` and `upgrade_page.rs` modules need this. It is UI-layer
  infrastructure, not business logic.
- `src/runner.rs` handles command execution and is already imported by backends; mixing
  in a UI-layer spawn helper would blur its purpose.
- Creating a new `src/spawn.rs` module is unnecessary overhead for a two-use helper.

---

## 4. Implementation Steps

### Step 1 — Add the helper to `src/ui/mod.rs`

**Before** (`src/ui/mod.rs`, complete file):

```rust
pub mod log_panel;
pub mod reboot_dialog;
pub mod update_row;
pub mod upgrade_page;
pub mod window;
```

**After:**

```rust
pub mod log_panel;
pub mod reboot_dialog;
pub mod update_row;
pub mod upgrade_page;
pub mod window;

use std::future::Future;

/// Spawns a new OS thread, creates a single-threaded Tokio runtime on it,
/// and drives the future returned by `f` to completion via `block_on`.
///
/// If the Tokio runtime fails to build (resource exhaustion — effectively
/// unreachable in normal operation), an error is printed to stderr and the
/// thread exits without producing any output on caller channels.
pub(crate) fn spawn_background_async<F, Fut>(f: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
{
    std::thread::spawn(move || {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => {
                rt.block_on(f());
            }
            Err(e) => {
                eprintln!("Failed to build Tokio runtime: {e}");
            }
        }
    });
}
```

No new `Cargo.toml` dependencies. `std::future::Future` is part of the standard library.

---

### Step 2 — Refactor Occurrence A in `src/ui/window.rs`

**Location:** Inside `glib::spawn_future_local(async move { ... })` in
`build_update_page`, triggered by the Update All button.

**Before** (the `std::thread::spawn` block and its surrounding channel setup):

```rust
                // Clone senders for the worker thread
                let tx_thread = tx.clone();
                let result_tx_thread = result_tx.clone();
                let backends_thread = backends.clone();

                std::thread::spawn(move || {
                    match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(rt) => {
                            rt.block_on(async {
                                for backend in &backends_thread {
                                    let kind = backend.kind();
                                    let runner = CommandRunner::new(tx_thread.clone(), kind);
                                    let result = backend.run_update(&runner).await;
                                    let _ = result_tx_thread.send((kind, result)).await;
                                }
                            });
                        }
                        Err(e) => {
                            // Send an error result for every backend so the UI exits its recv loop
                            for backend in &backends_thread {
                                let kind = backend.kind();
                                let _ = result_tx_thread.send_blocking((
                                    kind,
                                    crate::backends::UpdateResult::Error(format!(
                                        "Runtime error: {e}"
                                    )),
                                ));
                            }
                        }
                    }

                    drop(tx_thread);
                    drop(result_tx_thread);
                });
```

**After:**

```rust
                // Clone senders for the worker thread
                let tx_thread = tx.clone();
                let result_tx_thread = result_tx.clone();
                let backends_thread = backends.clone();

                super::spawn_background_async(move || async move {
                    for backend in &backends_thread {
                        let kind = backend.kind();
                        let runner = CommandRunner::new(tx_thread.clone(), kind);
                        let result = backend.run_update(&runner).await;
                        let _ = result_tx_thread.send((kind, result)).await;
                    }
                });
```

Changes:
- Deleted the `match tokio::runtime::Builder::new_current_thread()...` block entirely.
- Deleted the `Err(e)` error path (runtime build failure — see §3.1 for rationale).
- Deleted the redundant `drop(tx_thread)` and `drop(result_tx_thread)` lines.
- The `async { ... }` block becomes `async move { ... }` because the variables are now
  moved into the future (owned by it) rather than borrowed from the enclosing closure.
- No `use` import needed: `super::spawn_background_async` is resolved via `super`.

---

### Step 3 — Refactor Occurrence B in `src/ui/window.rs`

**Location:** Inside `run_checks: Rc<dyn Fn()>`, inside the per-backend
`glib::spawn_future_local(async move { ... })` loop.

**Before:**

```rust
                        std::thread::spawn(move || {
                            match tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                            {
                                Ok(rt) => {
                                    rt.block_on(async {
                                        let result = backend_clone.count_available().await;
                                        let _ = tx.send(result).await;
                                    });
                                }
                                Err(e) => {
                                    let _ = tx.send_blocking(Err(format!("Runtime error: {e}")));
                                }
                            }
                        });
```

**After:**

```rust
                        super::spawn_background_async(move || async move {
                            let result = backend_clone.count_available().await;
                            let _ = tx.send(result).await;
                        });
```

Changes:
- Same structure as Occurrence A: deleted match block, deleted error path, flattened
  to a single `async move { ... }` closure.

---

### Step 4 — `use` imports audit

| File | Import to add | Import to remove |
|------|--------------|-----------------|
| `src/ui/mod.rs` | `use std::future::Future;` | — |
| `src/ui/window.rs` | — | None needed; all existing `use` statements remain valid. `tokio::runtime::Builder` was referenced inline (not via a `use`); no `use tokio` import exists in `window.rs` that would become unused. |

Verify after the change that `cargo build` emits no `unused_imports` warnings.

---

## 5. Dependencies

No new `Cargo.toml` entries required.

`tokio` is already a dependency (used in `src/runner.rs` and transitively). The
`std::future::Future` trait is in the standard library.

---

## 6. Affected Files

| File | Change |
|------|--------|
| `src/ui/mod.rs` | Add `use std::future::Future;` + `spawn_background_async` function body |
| `src/ui/window.rs` | Replace 2 × Tokio runtime boilerplate blocks with `super::spawn_background_async(...)` calls |
| `src/ui/upgrade_page.rs` | **No changes.** Its `std::thread::spawn` calls are synchronous and out of scope. |

---

## 7. Risks and Mitigations

### Risk 1: Async block lifetime / `'static` bound at call sites

**Concern:** `backend.run_update(&runner)` returns
`Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>>` with an explicit lifetime `'a`
tied to `&backend` (borrow of `backends_thread`) and `&runner` (loop-local).
If the compiler infers the outer `async move { ... }` block as non-`'static` due to
this internally-referenced `'a`, the call may fail to satisfy
`F: FnOnce() -> Fut + Send + 'static` where the relevant constraint is on `F`, not `Fut`.

**Analysis:** The helper's `Fut` is explicitly NOT constrained as `'static`. The `'static`
constraint is only on `F` (the closure). Since `F` captures only `'static` data
(`Vec<Box<dyn Backend + 'static>>`, `async_channel::Sender<T>`), `F: 'static` holds.
The future `Fut` itself lives entirely within the spawned thread; `block_on` imposes no
`'static` requirement on it. Internal self-referential borrows in the async state machine
are expected to be handled by Rust's async desugaring machinery.

**Mitigation:** If compilation fails with a lifetime error on the `run_update` line,
the async block can be revised to avoid hoisting the reference across an `.await` — for
example, by introducing a local binding that takes ownership of the needed data before
calling `run_update`. This is a localised code change and does not affect the helper
signature.

### Risk 2: Error path behavior change for runtime build failure

**Concern:** Option A removes the existing error paths that send channel messages on
`tokio::runtime::Builder::build()` failure. This changes observable UI behaviour.

**Affected path:** `tokio::runtime::Builder::new_current_thread().enable_all().build()`
failing on a healthy Linux system.

**Probability:** Effectively zero in production (requires OS-level `pthread_create`
exhaustion or equivalent).

**Mitigation:** The change is documented explicitly in §3.1. If the team decides that
exact error-path fidelity is required, Option B can be implemented by adding an
`on_error: impl FnOnce(io::Error) + Send + 'static` parameter to the helper. This is
a compatible API extension requiring no changes to the helper's internal structure.

### Risk 3: `upgrade_page.rs` synchronous spawns may be mistaken as in-scope

**Concern:** Future contributors may assume all `std::thread::spawn` calls in `window.rs`
and `upgrade_page.rs` use the helper, leading to incorrect refactoring of the
synchronous spawn pattern in `upgrade_page.rs`.

**Mitigation:** The helper's doc comment explicitly states it is for async work requiring
a Tokio runtime. The synchronous spawns in `upgrade_page.rs` have a distinctly different
shape (no `.await`, `send_blocking` everywhere) and are easily recognised as separate.

---

## 8. Summary

There are **exactly 2 occurrences** of the `std::thread::spawn` +
`tokio::runtime::Builder::new_current_thread()` + `rt.block_on` pattern; both are in
`src/ui/window.rs`. The `upgrade_page.rs` file contains 3 `std::thread::spawn` calls but
none use a Tokio runtime — they are out of scope.

A generic free function `spawn_background_async<F, Fut>(f: F)` with constraints
`F: FnOnce() -> Fut + Send + 'static` and `Fut: Future<Output = ()>` (no `'static` or
`Send` on `Fut`) captures both call sites cleanly. The function lives in `src/ui/mod.rs`.

Option A (runtime build errors logged to stderr) is recommended. The existing error
paths handle a virtually-unreachable condition inconsistently; Option A is simpler and
more honest about the invariant.

The only files changed are `src/ui/mod.rs` (add helper) and `src/ui/window.rs` (2 × call
site replacement). No new Cargo dependencies are introduced.
