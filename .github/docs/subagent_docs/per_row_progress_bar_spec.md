# Per-Row Progress Bar Fix — Specification

**Feature:** `per_row_progress_bar`  
**Date:** 2026-04-19  
**Status:** READY FOR IMPLEMENTATION

---

## 1. Current State Analysis

### How progress bars are created and managed

Each `UpdateRow` (defined in `src/ui/update_row.rs`) owns a private `gtk::ProgressBar` and a `glib::SourceId`-based timer stored in `progress_timer: Rc<RefCell<Option<glib::SourceId>>>`.

`set_status_running()` starts the progress animation:

```rust
// src/ui/update_row.rs  ~line 120
pub fn set_status_running(&self) {
    if let Some(source_id) = self.progress_timer.borrow_mut().take() {
        source_id.remove();   // cancel previous timer
    }
    self.progress_fraction.set(0.0);
    self.spinner.set_visible(true);
    self.spinner.set_spinning(true);
    self.progress_bar.set_visible(true);
    self.progress_bar.set_fraction(0.0);
    self.status_label.set_label("Updating...");

    let source_id = glib::timeout_add_local(Duration::from_millis(200), move || {
        // increments fraction by 0.005 every 200 ms, caps at 0.95
        ...
        glib::ControlFlow::Continue
    });
    *self.progress_timer.borrow_mut() = Some(source_id);
}
```

The timer runs indefinitely until `stop_progress_timer()` is called (via `set_status_success`, `set_status_error`, or `set_status_skipped`).

### How backend execution is triggered

Backends run **sequentially** inside a single `spawn_background_async` block in `src/ui/window.rs`, inside the `update_button.connect_clicked` handler:

```rust
// src/ui/window.rs  ~line 290 (inside spawn_background_async closure)
for backend in &ordered_backends {
    let kind = backend.kind();
    let runner = CommandRunner::new(tx_thread.clone(), kind, shell.clone());
    let result = backend.run_update(&runner).await;   // ← sequential, one at a time
    let _ = result_tx_thread.send((kind, result)).await;
}
```

Backends are sorted so that privileged backends (needs_root == true) run first.

### How progress/completion signals flow from backends → channels → UI

Two async channels carry information from the background thread to the GTK main thread:

| Channel | Type | Purpose |
|---------|------|---------|
| `tx` / `rx` | `async_channel::unbounded::<(BackendKind, String)>` | Streams log lines per backend |
| `result_tx` / `result_rx` | `async_channel::unbounded::<(BackendKind, UpdateResult)>` | Delivers final result per backend |

The GTK main thread processes these in two concurrent `glib::spawn_future_local` tasks:

1. **Log task** — receives `(kind, line)` from `rx` and appends to the log panel.
2. **Result task** — receives `(kind, result)` from `result_rx` and calls the appropriate `set_status_*` method on the matching row.

---

## 2. Root Cause of the Bug

**Location:** `src/ui/window.rs`, inside the `glib::spawn_future_local` block in `update_button.connect_clicked`, immediately after authentication succeeds.

**The offending code (~line 355):**

```rust
// --- Begin updates ---
status_ref.set_label("Updating\u{2026}");
{
    let rows_borrowed = rows_ref.borrow();
    for (_, row) in rows_borrowed.iter() {
        row.set_status_running();   // ← CALLED ON ALL ROWS AT ONCE
    }
}
```

This loop iterates over **every** `UpdateRow` and calls `set_status_running()` on all of them **before any backend has even started executing**. Because `set_status_running()` immediately starts a repeating `glib::timeout_add_local` timer for each row, all progress bars begin animating simultaneously.

The backends themselves execute correctly one-at-a-time (sequentially), but the UI is already ahead of them — all rows show "Updating…" with running progress bars from the first moment.

**Why the result channel alone cannot fix this:**  
The result channel only fires *after* a backend completes. Without a "started" notification, there is no signal to tell the UI which specific row to activate at the moment that backend begins.

---

## 3. Proposed Fix Architecture

### Core idea

Introduce a third lightweight channel — `started_tx` / `started_rx` of type `async_channel::unbounded::<BackendKind>()` — that sends the `BackendKind` of whichever backend is *about to start* executing. The GTK main thread receives from this channel and calls `set_status_running()` only on the matching row.

Remove the existing pre-loop that calls `set_status_running()` on all rows.

### Data flow after the fix

```
Background thread (spawn_background_async):
  for each backend (sequentially):
    1.  started_tx.send(kind)         → GTK: set THAT row to "running"
    2.  backend.run_update(&runner)   → streams log lines via tx
    3.  result_tx.send((kind, result)) → GTK: set THAT row to success/error/skipped

GTK main thread (glib::spawn_future_local tasks):
  Task A — started_rx → set_status_running() for matching row
  Task B — rx        → append log line
  Task C — result_rx → set_status_success/error/skipped for matching row
```

### Why a new channel (not repurposing existing ones)?

- The log channel `tx` carries `(BackendKind, String)` text output. Encoding lifecycle events as magic strings would be fragile and untestable.
- The result channel `result_tx` carries `UpdateResult` (an outcome type). Mixing in a `Started` variant would semantically pollute the result type and every match arm throughout the codebase.
- A dedicated `started_tx: async_channel::Sender<BackendKind>` requires no new types — `BackendKind` already derives `Clone`, `Copy`, `PartialEq`, and `Eq`.

---

## 4. Exact Implementation Steps

All changes are confined to **`src/ui/window.rs`**.

### Step 1 — Create the `started` channel alongside the existing channels

Find the block where `tx`/`rx` and `result_tx`/`result_rx` are created:

```rust
let (tx, rx) = async_channel::unbounded::<(BackendKind, String)>();
let (result_tx, result_rx) =
    async_channel::unbounded::<(BackendKind, UpdateResult)>();
```

Add after these two lines:

```rust
let (started_tx, started_rx) =
    async_channel::unbounded::<BackendKind>();
```

### Step 2 — Clone `started_tx` into the background thread

After `let tx_thread = tx.clone();` and `let result_tx_thread = result_tx.clone();`, add:

```rust
let started_tx_thread = started_tx.clone();
```

### Step 3 — Send "started" before each `run_update` in the background loop

Inside the `spawn_background_async` closure, change the backend loop from:

```rust
for backend in &ordered_backends {
    let kind = backend.kind();
    let runner = CommandRunner::new(tx_thread.clone(), kind, shell.clone());
    let result = backend.run_update(&runner).await;
    let _ = result_tx_thread.send((kind, result)).await;
}
```

to:

```rust
for backend in &ordered_backends {
    let kind = backend.kind();
    let _ = started_tx_thread.send(kind).await;
    let runner = CommandRunner::new(tx_thread.clone(), kind, shell.clone());
    let result = backend.run_update(&runner).await;
    let _ = result_tx_thread.send((kind, result)).await;
}
```

### Step 4 — Drop `started_tx` alongside the other senders

Find the existing drop block:

```rust
drop(tx);
drop(result_tx);
```

Add:

```rust
drop(started_tx);
```

### Step 5 — Remove the bulk `set_status_running()` pre-loop

Find and remove the following block (it appears after the authentication success branch):

```rust
// --- Begin updates ---
status_ref.set_label("Updating\u{2026}");
{
    let rows_borrowed = rows_ref.borrow();
    for (_, row) in rows_borrowed.iter() {
        row.set_status_running();
    }
}
```

Replace it with just the status label update (no row loop):

```rust
// --- Begin updates ---
status_ref.set_label("Updating\u{2026}");
```

### Step 6 — Add a GTK task to handle `started_rx`

After the log-streaming task (which receives from `rx`), add a new `glib::spawn_future_local` task that handles `started_rx`:

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

This task runs concurrently with the log task and the result task. All three live on the GTK main thread (single-threaded), so no lock contention is possible. Borrows of `rows_ref` are short-lived and always dropped before each `.await`, preventing borrow conflicts.

---

## 5. Files That Need to Change

| File | Change |
|------|--------|
| `src/ui/window.rs` | Add `started` channel, remove bulk `set_status_running()` loop, add per-row started handling task |

No other files require modification. The `UpdateRow` struct, `set_status_running()`, all backends, and the runner are correct as-is.

---

## 6. Sequence Diagram (After Fix)

```
GTK thread                    Background thread
    │                                │
    │   [Update All clicked]         │
    │──────────────────────────────► │
    │                                │
    │   [auth]                       │
    │◄──────────────────────────────-│
    │                                │
    │   status = "Updating…"         │
    │                                │
    │                          started_tx.send(Nix)
    │◄─────────────────────────────-─│
    │   row[Nix].set_status_running()│
    │   (Nix bar starts filling)     │
    │                                │
    │                          Nix runs…
    │◄─── log lines (Nix) ──────────-│
    │                                │
    │                          result_tx.send(Nix, Success)
    │◄──────────────────────────────-│
    │   row[Nix].set_status_success()│
    │   (Nix bar fills to 100%, hides)│
    │                                │
    │                          started_tx.send(Flatpak)
    │◄──────────────────────────────-│
    │   row[Flatpak].set_status_running()
    │   (Flatpak bar starts filling) │
    │                                │
    │                          Flatpak runs…
    │◄─── log lines (Flatpak) ──────-│
    │                                │
    │                          result_tx.send(Flatpak, Success)
    │◄──────────────────────────────-│
    │   row[Flatpak].set_status_success()
    │   (Flatpak bar fills to 100%, hides)
    │                                │
    │   status = "Update complete."  │
```

---

## 7. Risks and Mitigations

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| `started_rx` task borrows `rows_ref` while `result_rx` task holds a borrow | Low | Both tasks run on the same GTK main loop thread; borrows are always dropped before `.await` so there is no overlap |
| `started_tx_thread` not dropped, channel never closes, task leaks | Low | `started_tx` is explicitly `drop()`-ed alongside `tx` and `result_tx` (Step 4); the `started_tx_thread` clone is moved into the background closure and dropped when the closure completes |
| Channel send fails | Negligible | `started_tx.send()` returns `Err` only when all receivers have been dropped, which cannot happen before the GTK task terminates; the `let _ =` pattern silently discards this impossible error, consistent with the existing pattern for `tx` and `result_tx` |
| Regression: rows that have no updates are shown as "running" | None | The started notification is sent only immediately before `run_update()` is called; if a backend is skipped via `UpdateResult::Skipped`, the row still transitions through "running" briefly before `set_status_skipped()` is called — this is correct and consistent with current UX expectations |

---

## 8. Summary

**Root cause:** `src/ui/window.rs` calls `set_status_running()` on *all* rows simultaneously before any backend executes, starting all progress timers at once.

**Fix:** Add a `started_tx: async_channel::Sender<BackendKind>` channel; send the backend kind *just before* `run_update()` in the background loop; in the GTK main loop, listen for these signals and call `set_status_running()` only on the matching row; remove the bulk pre-loop.

**Scope:** Single file — `src/ui/window.rs`. No API changes required.
