# Per-Row Progress Bar Fix — Review

**Feature:** `per_row_progress_bar`  
**Date:** 2026-04-19  
**Reviewer:** QA Subagent  
**Status:** NEEDS_REFINEMENT

---

## Build Output Summary

| Command | Result |
|---------|--------|
| `cargo build` | ✅ PASS — `Finished 'dev' profile` in 3.88s, zero errors |
| `cargo clippy -- -D warnings` | ✅ PASS — zero warnings |
| `cargo fmt --check` | ❌ FAIL — formatting diff detected |

### `cargo fmt` diff

```
Diff in /home/nimda/Projects/Up/src/ui/window.rs:266:
                 let (tx, rx) = async_channel::unbounded::<(BackendKind, String)>();
                 let (result_tx, result_rx) =
                     async_channel::unbounded::<(BackendKind, UpdateResult)>();
-                let (started_tx, started_rx) =
-                    async_channel::unbounded::<BackendKind>();
+                let (started_tx, started_rx) = async_channel::unbounded::<BackendKind>();
```

**Root cause:** The `started_tx`/`started_rx` channel declaration was written across two lines, mirroring the style of the longer `result_tx`/`result_rx` declaration. However, `async_channel::unbounded::<BackendKind>()` fits on a single line with the binding and `rustfmt` collapses it.

---

## Review Checklist

### 1. Spec Compliance — ✅ PASS (with formatting note)

All five implementation steps from the spec are present:

| Spec Step | Status |
|-----------|--------|
| Step 1: Create `started` channel with `unbounded::<BackendKind>()` | ✅ Present |
| Step 2: Clone `started_tx` as `started_tx_thread` into background thread | ✅ Present |
| Step 3: Send `started_tx_thread.send(kind)` before `run_update` in loop | ✅ Present |
| Step 4: Drop `started_tx` alongside `tx` and `result_tx` | ✅ Present |
| Step 5: Remove bulk `set_status_running()` pre-loop | ✅ Removed |
| Step 6: Add `glib::spawn_future_local` task receiving from `started_rx` | ✅ Present |

---

### 2. Root Cause Fix — ✅ PASS

The offending pre-loop block:

```rust
// --- Begin updates ---
status_ref.set_label("Updating…");
{
    let rows_borrowed = rows_ref.borrow();
    for (_, row) in rows_borrowed.iter() {
        row.set_status_running(); // ← was here, now removed
    }
}
```

…has been replaced with only the status label update:

```rust
// --- Begin updates ---
status_ref.set_label("Updating\u{2026}");
```

The bulk `set_status_running()` pre-loop is gone.

---

### 3. New Channel — ✅ PASS

`started_tx`/`started_rx` are correctly typed as `async_channel::unbounded::<BackendKind>()` at lines 269–270 of `src/ui/window.rs`.

---

### 4. Signal Timing — ✅ PASS

Inside the `spawn_background_async` closure, `started_tx_thread.send(kind).await` is called **before** `backend.run_update(&runner).await`:

```rust
for backend in &ordered_backends {
    let kind = backend.kind();
    let _ = started_tx_thread.send(kind).await;           // ← BEFORE run_update
    let runner = CommandRunner::new(tx_thread.clone(), kind, shell.clone());
    let result = backend.run_update(&runner).await;
    let _ = result_tx_thread.send((kind, result)).await;
}
```

This is correct.

---

### 5. GTK Task — ✅ PASS

A dedicated `glib::spawn_future_local` task processes `started_rx` and calls `set_status_running()` only on the matching row:

```rust
let rows_for_started = rows_ref.clone();
glib::spawn_future_local(async move {
    while let Ok(kind) = started_rx.recv().await {
        let rows_borrowed = rows_for_started.borrow();
        if let Some((_, row)) = rows_borrowed.iter().find(|(k, _)| *k == kind) {
            row.set_status_running();
        }
    }
});
```

This task is spawned before the log task, with correct per-row matching by `BackendKind`.

---

### 6. Channel Cleanup — ✅ PASS

All three non-thread senders are explicitly dropped after the background closure is spawned:

```rust
drop(tx);
drop(result_tx);
drop(started_tx);
```

`started_tx_thread` (the clone moved into the background thread) is dropped automatically when the closure completes. No channel leak.

---

### 7. Code Quality — ❌ FAIL

- `cargo fmt --check` fails with one diff (see Build Output Summary above).
- The multi-line channel initialization at line 269–270 must be collapsed to a single line to satisfy `rustfmt`.
- No dead code, no unused imports, and `clippy` reports zero warnings.

---

### 8. Existing Channels Preserved — ✅ PASS

- **Log channel** (`tx`/`rx`): The `glib::spawn_future_local` log task (`while let Ok((kind, line)) = rx.recv().await`) is intact and unchanged.
- **Result channel** (`result_tx`/`result_rx`): The result processing loop (`while let Ok((kind, result)) = result_rx.recv().await`) is intact with all `UpdateResult` match arms unchanged.
- `auth_status_tx`/`auth_status_rx` bounded channel for authentication gating is also unchanged.

---

## Findings Summary

| Finding | Severity | Status |
|---------|----------|--------|
| `cargo fmt --check` fails — `started_tx`/`started_rx` declaration spans two lines instead of one | **CRITICAL** | ❌ Must fix |
| All other spec items correctly implemented | — | ✅ |
| `cargo build` clean | — | ✅ |
| `cargo clippy -- -D warnings` clean | — | ✅ |

---

## Required Fix

In `src/ui/window.rs`, change lines 269–270 from:

```rust
let (started_tx, started_rx) =
    async_channel::unbounded::<BackendKind>();
```

to:

```rust
let (started_tx, started_rx) = async_channel::unbounded::<BackendKind>();
```

---

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 98% | A |
| Best Practices | 95% | A |
| Functionality | 100% | A+ |
| Code Quality | 70% | C |
| Security | 100% | A+ |
| Performance | 100% | A+ |
| Consistency | 95% | A |
| Build Success | 67% | D |

> Build Success is 67% because 2 of 3 required commands pass (`cargo build`, `cargo clippy`) and 1 fails (`cargo fmt --check`).  
> Code Quality is penalised for the formatting failure.

**Overall Grade: B (90.6%)**  
*(Score weighted by severity: Functionality/Security/Performance at full weight; Build Success and Code Quality at 2× weight.)*

---

## Verdict

**NEEDS_REFINEMENT**

The implementation is functionally correct and architecturally sound. The single required fix is trivial: collapse the two-line `started_tx`/`started_rx` channel declaration onto one line to satisfy `rustfmt`.
