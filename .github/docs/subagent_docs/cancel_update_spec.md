# Cancel Running Update — Implementation Specification

**Feature**: Cancel running update — close privileged shell stdin; propagate `Cancelled` to each row  
**Date**: 2026-05-08  
**Status**: SPECIFICATION (Phase 1)

---

## 1. Current State Analysis

### 1.1 Row State Model

`UpdateRow` (in `src/ui/update_row.rs`) does **not** use a named `RowState` enum. Instead, state
transitions are expressed entirely as imperative `set_status_*` methods. The complete set as of
the current codebase:

| Method | Label | CSS Class | Spinner | Retry Visible |
|--------|-------|-----------|---------|---------------|
| `set_status_checking()` | `"Checking..."` | `dim-label` | on | no |
| `set_status_available(n)` | `"Up to date"` / `"{n} available"` | `success` / `accent` | off | no |
| `set_status_running()` | `"Updating..."` | `accent` | on | no |
| `set_status_success(n)` | `"Up to date"` / `"{n} updated"` | `success` | off | no |
| `set_status_error(msg)` | `"Error: {msg}"` | `error` | off | **yes** |
| `set_status_skipped(msg)` | *(msg)* | `dim-label` | off | no |
| `set_status_unknown(msg)` | *(msg)* | `dim-label` | off | no |
| `set_status_cleaning()` | `"Cleaning…"` | `accent` | on | no |
| `set_status_cleaned(n)` | `"Already clean"` / `"{n} removed"` | `success` | off | no |

**There is no `Cancelled` status.** It must be added.

### 1.2 `UpdateResult` (verbatim from `src/backends/mod.rs`)

```rust
pub enum UpdateResult {
    Success {
        updated_count: usize,
    },
    SuccessWithSelfUpdate {
        updated_count: usize,
    },
    Error(BackendError),
    #[allow(dead_code)]
    Skipped(String),
}
```

**There is no `Cancelled` variant.** It must be added.

### 1.3 `BackendError` (verbatim from `src/backends/mod.rs`)

```rust
#[derive(Debug, thiserror::Error, Clone)]
pub enum BackendError {
    #[error("Authentication cancelled or denied")]
    AuthCancelled,
    #[error("Failed to spawn process: {0}")]
    Spawn(String),
    #[error("Command failed (exit {code}): {message}")]
    Exit { code: i32, message: String },
    #[error("Failed to parse command output: {0}")]
    #[allow(dead_code)]
    Parse(String),
    #[error("Network error: {0}")]
    #[allow(dead_code)]
    Network(String),
}
```

**There is no `Cancelled` variant.** One must be added.

### 1.4 `PrivilegedShell` Architecture

```rust
pub struct PrivilegedShell {
    child: tokio::process::Child,
    stdin: Option<tokio::process::ChildStdin>,
    reader: BufReader<tokio::process::ChildStdout>,
    session_id: String,
}
```

Key behaviours:
- Commands are sent **one at a time**: a single `run_command` call writes a command + sentinel
  probe to stdin and then enters a read loop that blocks until the `___UP_RC_<n>___` sentinel
  appears on stdout.
- **`close()` method** drops `stdin` (sends EOF) and then calls `child.wait()`. This causes `sh`
  to exit after it processes the EOF. If a command is mid-execution when stdin is closed, the
  shell will finish the current command and then exit when it reaches EOF, OR the `read_line`
  returns `0` bytes (pipe closed from the child side) causing:
  ```
  return Err("Privileged shell closed unexpectedly".to_string());
  ```
  This propagates as `BackendError::Exit { code: -1, message: "..." }`.
- There is a 1-hour `COMMAND_TIMEOUT` via `tokio::time::timeout`; on timeout the shell is
  closed.
- **No mid-command abort exists** other than closing stdin / killing the process.

### 1.5 Orchestrator Architecture

The `UpdateOrchestrator::run_all` method runs backends **sequentially** in a `for` loop:

```rust
for backend in &backends {
    let kind = backend.kind();
    let _ = tx.send(OrchestratorEvent::BackendStarted(kind)).await;
    let runner = CommandRunner::new(be_tx.clone(), kind, shell.clone());
    let result = backend.run_update(&runner).await;
    let _ = tx.send(OrchestratorEvent::BackendFinished(kind, result)).await;
}
```

The entire loop runs in a single Tokio task spawned via
`crate::runtime::runtime().spawn(...)`.

### 1.6 Existing Cancellation Infrastructure

**None.** There is no `CancellationToken`, `AtomicBool`, `watch` channel, or `mpsc` abort
signal anywhere in the codebase. The `update_in_progress: Rc<Cell<bool>>` and
`updating: Rc<Cell<bool>>` are reentrancy guards only.

### 1.7 Cancel Button in UI

**None exists.** The "Update All" button is simply disabled (`button.set_sensitive(false)`)
when an update is running. There is no cancel affordance.

### 1.8 `tokio-util` Dependency

`tokio-util` is **not** in `Cargo.toml`. The current `tokio` dependency includes:
`rt`, `rt-multi-thread`, `macros`, `io-util`, `process`, `fs`, `sync`, `time`.
The `sync` feature includes `tokio::sync::watch`, `tokio::sync::Mutex`, and `std::sync::atomic`
is available from `std`.

---

## 2. Feature Definition

### 2.1 User Story

> As a user, I want to cancel an in-progress update by clicking a "Cancel" button, so that I
> can stop the update safely without leaving a zombie root shell running.

### 2.2 Behaviour Contract

1. A **Cancel button** appears in the UI when an update is running.
2. Clicking Cancel:
   a. Signals the orchestrator to stop.
   b. If a root command is running inside `PrivilegedShell`, closes its stdin immediately,
      causing the shell to exit (hard cancel for root backends).
   c. If a non-root backend is running (Flatpak, Homebrew, Nix), the current command runs to
      completion (soft cancel), then remaining backends are skipped.
   d. All **not-yet-started** rows receive `Cancelled` status.
   e. The **currently-running** row receives `Cancelled` status (even if the raw command
      returned an error, the cancel flag takes precedence).
   f. Already-finished rows retain their status (Success / Error / Skipped).
3. The "Update All" button is re-enabled after cancel completes.
4. The Cancel button is hidden once the update ends (normally or by cancellation).
5. History entries are NOT written for `Cancelled` backends (they did not complete).
6. The privileged `sh` process is fully terminated — no zombie.

---

## 3. Architecture Decision

### 3.1 Options Evaluated

#### Option A: `tokio_util::CancellationToken`
Thread a `CancellationToken` through every `run_update` call via trait signature change. Use
`tokio::select!` at each async await point in backends and `run_command`.

- **Pro**: Idiomatic cooperative cancellation; precise cancel points.
- **Con**: Requires adding `tokio-util` crate dependency (not in Cargo.toml). Requires
  modifying the `Backend` trait (`run_update` signature) and `CommandExecutor` trait — breaking
  changes touching every backend. The `PrivilegedShell::run_command` read loop has no natural
  cancel point without a `select!` rewrite.

#### Option B: `tokio::sync::watch` channel
Broadcast a `bool` cancel signal. Backends poll it between subcommand steps.

- **Pro**: No new crate. `tokio::sync::watch` is already available.
- **Con**: Same Backend/CommandExecutor trait signature change required. Still cannot interrupt
  `run_command`'s synchronous read loop mid-sentinel-wait. `borrow()` polling between steps is
  adequate only for multi-step backends; single-command backends (apt, dnf) cannot check it
  mid-run.

#### Option C: Close stdin + `Arc<AtomicBool>` cancel flag ✅ **CHOSEN**

- Create a `CancelHandle` containing:
  - `Arc<AtomicBool>` — the cancel flag (no new crate, no trait changes)
  - `Arc<std::sync::Mutex<Option<Arc<tokio::sync::Mutex<PrivilegedShell>>>>>` — a shared slot
    populated once the shell is created, used to call `close()` from the cancel handler
- `run_all` returns a `CancelHandle`.
- When cancel is clicked: sets the flag + spawns a Tokio task to close the shell.
- In the orchestrator's backend loop:
  - Check the flag **before** each backend. If set → emit `BackendFinished(kind,
    UpdateResult::Cancelled)` and break.
  - Check the flag **after** each `BackendFinished`. If set → emit `Cancelled` for remaining
    backends.
  - When a backend returns `UpdateResult::Error(_)` AND the flag is set → override to
    `UpdateResult::Cancelled` (the error was caused by the forced shell close).
- **No trait changes** to `Backend`, `CommandExecutor`, or any backend implementation.
- **No new crate dependency**.

### 3.2 Justification

| Criterion | Option A | Option B | Option C |
|-----------|----------|----------|----------|
| New crate dependency | ✗ (tokio-util) | ✓ | ✓ |
| Backend trait unchanged | ✗ | ✗ | ✓ |
| Shell killed immediately for root cmds | ✓ (select!) | ✗ | ✓ (stdin close) |
| Non-root soft cancel | ✓ | ✓ | ✓ (wait for cmd) |
| No zombie root shell | ✓ | ~ | ✓ (`close()` awaits child) |
| Minimal invasiveness | ✗ | ✗ | ✓ |

Option C is the correct choice: safest, zero new dependencies, zero trait-signature churn.

---

## 4. `UpdateRow` Cancelled Visual Design

### 4.1 New Method: `set_status_cancelled()`

```rust
pub fn set_status_cancelled(&self) {
    self.retry_button.set_visible(false);
    self.skip_checkbox.set_sensitive(true);
    self.spinner.set_visible(false);
    self.spinner.set_spinning(false);
    self.status_label.set_label("Cancelled");
    self.status_label.set_css_classes(&["dim-label"]);
}
```

Design rationale:
- **"Cancelled"** — explicit, unambiguous, consistent with GNOME HIG language.
- **`dim-label`** — same CSS class as `Skipped`, communicating "nothing happened" without alarm.
  Does NOT use `error` (red), because the user chose to cancel; it is not a failure.
- **No retry button** — the user explicitly stopped. They can click "Update All" again.
- **Spinner off** — work stopped.

---

## 5. New Types

### 5.1 `UpdateResult::Cancelled`

In `src/backends/mod.rs`:

```rust
pub enum UpdateResult {
    Success { updated_count: usize },
    SuccessWithSelfUpdate { updated_count: usize },
    Error(BackendError),
    Skipped(String),
    /// The update was cancelled by the user before or during execution.
    Cancelled,
}
```

### 5.2 `BackendError::Cancelled`

In `src/backends/mod.rs`:

```rust
pub enum BackendError {
    AuthCancelled,
    Spawn(String),
    Exit { code: i32, message: String },
    Parse(String),
    Network(String),
    /// The update was cancelled by the user.
    #[error("Update cancelled by user")]
    Cancelled,
}
```

This is needed so `BackendError::from_string` can correctly classify shell-closed errors when
the cancel flag is set.

### 5.3 `OrchestratorEvent` — no new variant needed

The existing `BackendFinished(BackendKind, UpdateResult)` carries `UpdateResult::Cancelled`.
The UI already dispatches on `BackendFinished` and just needs a new match arm.

### 5.4 `CancelHandle`

In `src/orchestrator.rs` (new type, not a separate file):

```rust
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};

/// A lightweight cancel handle returned by `UpdateOrchestrator::run_all`.
///
/// Clone it freely; `cancel()` is safe to call from any thread (including
/// the GTK main thread).
#[derive(Clone)]
pub struct CancelHandle {
    cancelled: Arc<AtomicBool>,
    shell_slot: Arc<Mutex<Option<Arc<tokio::sync::Mutex<PrivilegedShell>>>>>,
}

impl CancelHandle {
    pub fn cancel(&self) {
        if self.cancelled.swap(true, Ordering::SeqCst) {
            return; // already cancelled
        }
        // Close the shell on a background task so we don't block the GTK thread.
        let slot = self.shell_slot.clone();
        crate::runtime::runtime().spawn(async move {
            let maybe_shell = {
                let mut guard = slot.lock().expect("shell_slot mutex poisoned");
                guard.take()
            };
            if let Some(shell_arc) = maybe_shell {
                shell_arc.lock().await.close().await;
            }
        });
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}
```

---

## 6. Implementation Steps (Ordered)

### Step 1 — `src/backends/mod.rs`: Add `Cancelled` to `BackendError` and `UpdateResult`

1a. Add to `BackendError` enum:
```rust
/// The update was cancelled by the user.
#[error("Update cancelled by user")]
Cancelled,
```

1b. Add to `BackendError::from_string()` — at the very top of the match logic, before other
checks (so it can be used from the orchestrator, not from the shell directly):
No change needed in `from_string`; classification happens at the orchestrator level.

1c. Add to `UpdateResult` enum:
```rust
/// The update was cancelled by the user before or during execution.
Cancelled,
```

### Step 2 — `src/orchestrator.rs`: Add `CancelHandle` struct and plumb into `run_all`

2a. Add `use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};` imports.

2b. Define `CancelHandle` struct (see §5.4 above) in this file.

2c. Change `UpdateOrchestrator::run_all` signature from:
```rust
pub fn run_all(&self, tx: async_channel::Sender<OrchestratorEvent>)
```
to:
```rust
pub fn run_all(&self, tx: async_channel::Sender<OrchestratorEvent>) -> CancelHandle
```

2d. Inside `run_all`, before `spawn_background`:
```rust
let cancelled = Arc::new(AtomicBool::new(false));
let shell_slot: Arc<Mutex<Option<Arc<tokio::sync::Mutex<PrivilegedShell>>>>> =
    Arc::new(Mutex::new(None));
let handle = CancelHandle {
    cancelled: cancelled.clone(),
    shell_slot: shell_slot.clone(),
};
```

2e. Return `handle` at the end of `run_all` (before or after `spawn_background` call — since
`spawn_background` is fire-and-forget, return happens synchronously after spawning):
```rust
spawn_background(move || async move { /* ... */ });
handle
```

2f. Inside the spawned async closure, after the shell is created successfully:
```rust
let shell: Option<Arc<tokio::sync::Mutex<PrivilegedShell>>> = if any_needs_root {
    // ... auth ...
    match PrivilegedShell::new().await {
        Ok(s) => {
            let arc = Arc::new(tokio::sync::Mutex::new(s));
            // Populate shell_slot so CancelHandle::cancel() can close it.
            if let Ok(mut guard) = shell_slot.lock() {
                *guard = Some(arc.clone());
            }
            Some(arc)
        }
        Err(e) => { /* ... auth failed ... */ return; }
    }
} else {
    None
};
```

2g. Change the backend iteration loop to check the cancel flag:

```rust
for backend in &backends {
    let kind = backend.kind();

    // Check for cancellation before starting each backend.
    if cancelled.load(Ordering::SeqCst) {
        let _ = tx.send(OrchestratorEvent::BackendFinished(kind, UpdateResult::Cancelled)).await;
        continue; // skip remaining backends
    }

    let _ = tx.send(OrchestratorEvent::BackendStarted(kind)).await;
    let runner = CommandRunner::new(be_tx.clone(), kind, shell.clone());
    let result = backend.run_update(&runner).await;

    // If user cancelled while this backend was running, override the result.
    let result = if cancelled.load(Ordering::SeqCst) {
        UpdateResult::Cancelled
    } else {
        result
    };

    let _ = tx.send(OrchestratorEvent::BackendFinished(kind, result)).await;
}
```

**Note on the `continue` vs `break` choice**: Using `continue` with an early `Cancelled` emit
for each remaining backend is preferable to `break`, because the UI event loop is expecting
`BackendFinished` for every backend it sees `BackendStarted` for. Since we skip
`BackendStarted` for cancelled backends, we only need to emit `Cancelled` for backends we
*would have started*. However, using `continue` with the early return inside the loop is clean:
we emit `BackendFinished(Cancelled)` for each not-yet-started backend, so the UI can mark them.

**Correction**: We must NOT emit `BackendStarted` if we skip a backend. The UI only sets a row
to `Running` on `BackendStarted`. So the correct sequence for cancelled remaining backends is:
`BackendFinished(kind, Cancelled)` only (no `BackendStarted`). The UI dispatch must handle
`BackendFinished` even when no corresponding `BackendStarted` was received.

2h. Do the same for `CleanupOrchestrator::run_all` — mirror the identical change. The
`CleanupOrchestrator` is used by the Maintenance action, which also benefits from cancel
capability. However, for the initial implementation, **only `UpdateOrchestrator` is updated**;
`CleanupOrchestrator` can be addressed separately. Document this as a known gap.

### Step 3 — `src/ui/update_row.rs`: Add `set_status_cancelled()`

Add the new method immediately after `set_status_skipped`:

```rust
pub fn set_status_cancelled(&self) {
    self.retry_button.set_visible(false);
    self.skip_checkbox.set_sensitive(true);
    self.spinner.set_visible(false);
    self.spinner.set_spinning(false);
    self.status_label.set_label("Cancelled");
    self.status_label.set_css_classes(&["dim-label"]);
}
```

### Step 4 — `src/ui/window.rs`: Add Cancel button + wire cancel logic

#### 4a. Create the Cancel button

In `build_update_page`, near where `update_button` is defined:

```rust
let cancel_button = gtk::Button::builder()
    .label("Cancel")
    .css_classes(vec!["destructive-action", "pill"])
    .halign(gtk::Align::Center)
    .margin_top(12)
    .visible(false)          // hidden until update starts
    .build();
cancel_button.update_property(&[gtk::accessible::Property::Label("Cancel update")]);
```

Append it to `content_box` right after `update_button`:
```rust
content_box.append(&update_button);
content_box.append(&cancel_button);
```

#### 4b. Hold a `CancelHandle` slot in the UI

Add a shared slot for the cancel handle:

```rust
let cancel_handle: Rc<RefCell<Option<crate::orchestrator::CancelHandle>>> =
    Rc::new(RefCell::new(None));
```

#### 4c. On "Update All" click — reveal Cancel, store handle

In the `update_button.connect_clicked` closure, after `button.set_sensitive(false)`:

```rust
cancel_button.set_visible(true);
```

After `orchestrator.run_all(event_tx)`:

```rust
let handle = orchestrator.run_all(event_tx);
*cancel_handle.borrow_mut() = Some(handle);
```

Modify the `glib::spawn_future_local` closure to capture `cancel_button` and `cancel_handle`:
```rust
#[strong] cancel_button,
#[strong] cancel_handle,
```

#### 4d. On "Update All" completion — hide Cancel, clear handle

At the end of the event loop (after `AllFinished` and after re-enabling `button`):

```rust
cancel_button.set_visible(false);
cancel_handle.borrow_mut().take(); // drop the handle
```

#### 4e. Wire the Cancel button click handler

```rust
cancel_button.connect_clicked(glib::clone!(
    #[strong] cancel_handle,
    move |btn| {
        btn.set_sensitive(false); // prevent double-click
        if let Some(handle) = cancel_handle.borrow().as_ref() {
            handle.cancel();
        }
    }
));
```

Note: after `handle.cancel()`, the background task will eventually complete and emit
`AllFinished`, which will re-enable the Update All button and hide Cancel via the existing
event loop. No additional UI cleanup is needed in the click handler itself.

#### 4f. Handle `UpdateResult::Cancelled` in the event dispatch loop

In the `OrchestratorEvent::BackendFinished` arm:

```rust
OrchestratorEvent::BackendFinished(kind, result) => {
    let rows_borrowed = rows.borrow();
    if let Some((_, row)) = rows_borrowed.iter().find(|(k, _)| *k == kind) {
        match &result {
            UpdateResult::Success { updated_count } => {
                row.set_status_success(*updated_count);
            }
            UpdateResult::SuccessWithSelfUpdate { updated_count } => {
                row.set_status_success(*updated_count);
                self_updated = true;
            }
            UpdateResult::Error(msg) => {
                row.set_status_error(&msg.to_string());
                has_error = true;
            }
            UpdateResult::Skipped(msg) => {
                row.set_status_skipped(msg);
            }
            UpdateResult::Cancelled => {             // NEW
                row.set_status_cancelled();
            }
        }
    }
    // History: do NOT append history entries for Cancelled results.
    if !matches!(result, UpdateResult::Cancelled) {
        let ts = crate::history::now_secs();
        // ... existing history entry creation ...
        history_entries.push(entry);
    }
}
```

Also handle `BackendFinished` arriving without a corresponding `BackendStarted` (for
not-yet-started cancelled backends): the UI code already does a `.find` lookup; if no row
matches a running state, `set_status_cancelled()` still works correctly since it just sets
label/spinner regardless of prior state.

### Step 5 — Status label when cancelled

In the completion block (after the event loop exits), add a cancelled path alongside the
existing error path:

```rust
let was_cancelled = cancel_handle.borrow().as_ref()
    .map(|h| h.is_cancelled())
    .unwrap_or(false);

if was_cancelled {
    status_label.set_label("Update cancelled.");
} else if has_error {
    status_label.set_label("Update completed with errors.");
} else {
    status_label.set_label("Update complete.");
}
```

### Step 6 — `UpdateResult` exhaustiveness: update all match sites

Run `cargo build` after Step 1c; the compiler will flag every non-exhaustive match on
`UpdateResult`. The additional match sites are:

- `src/orchestrator.rs` — none (doesn't match `UpdateResult`)
- `src/ui/window.rs` — two match sites: Update path (Step 4f above) and Maintenance path
  (add `UpdateResult::Cancelled => {}` arm for the cleanup orchestrator, which does not yet
  support cancel but must compile)
- Any future test code

---

## 7. New Dependencies

**None.** All required types are available in `std` (`Arc`, `Mutex`, `AtomicBool`) and in the
already-declared `tokio` crate (`tokio::sync::Mutex`, `tokio::runtime`). No changes to
`Cargo.toml` are required.

**Verification (Context7 / tokio docs)**:
- `tokio::sync::Mutex` — available since tokio 1.0; present in this project's tokio dep
  (version `"1"`) with `features = ["sync"]` ✓
- `std::sync::atomic::AtomicBool` with `Ordering::SeqCst` — stable since Rust 1.0 ✓
- `crate::runtime::runtime().spawn(...)` — already used in `orchestrator.rs` via
  `spawn_background` ✓

---

## 8. Risks & Mitigations

| Risk | Likelihood | Severity | Mitigation |
|------|-----------|----------|------------|
| Shell closes mid-command; apt/dnf left in partial state | Medium | Medium | This is inherent to hard-cancelling a root shell. Mitigated by the `sh` EOF behaviour: `sh` finishes the in-flight command line before exiting. Partial package states are recoverable via re-running the update. |
| `cancel()` called before shell is created (auth pending) | Low | Low | The `shell_slot` is `None` before auth completes. `cancel()` checks `guard.take()` which returns `None` — no panic. The `cancelled` flag is still set, so the orchestrator exits after auth. |
| `cancel()` called after update completes | Low | Low | `AtomicBool::swap` returns `true` if already cancelled; `CancelHandle::cancel()` returns early. The shell Arc has already been dropped (`.take()`d into `None` at completion or by a previous cancel). |
| GTK thread blocks on `cancel()` | None | N/A | `cancel()` is non-async; shell close is spawned on the Tokio runtime, not awaited on the GTK thread. |
| Double-click of Cancel button | Low | Low | Cancel button is set to `sensitive(false)` inside `connect_clicked` handler before calling `cancel()`. The `AtomicBool::swap` also guards against re-entry. |
| `BackendFinished(Cancelled)` without `BackendStarted` confuses UI | Low | Medium | The UI looks up rows by `BackendKind`, not by position. A `BackendFinished` with no prior `BackendStarted` for that kind is handled by `set_status_cancelled()` — it unconditionally sets the label regardless of prior state. |
| Non-root backend (Flatpak) takes long after cancel | Medium | Low | Accepted trade-off. The current Flatpak command runs to completion. User can see spinner on the row; Cancel button is still visible. The next backend is skipped. Could be addressed later by killing the child process (requires storing `Arc<Mutex<Option<Child>>>` inside `CommandRunner`). |
| History entries for partially-completed backends | Low | Low | Step 4f explicitly skips history writing for `Cancelled` results. |
| `CleanupOrchestrator` not updated | Low | Low | Maintenance action is not cancellable in this implementation. The `UpdateResult::Cancelled` arm is added as a no-op to keep it compiling. Documented as known gap. |

---

## 9. Files Modified

| File | Change |
|------|--------|
| `src/backends/mod.rs` | Add `BackendError::Cancelled`, `UpdateResult::Cancelled` |
| `src/orchestrator.rs` | Add `CancelHandle` struct; `run_all` returns `CancelHandle`; add cancel checks in backend loop |
| `src/ui/update_row.rs` | Add `set_status_cancelled()` |
| `src/ui/window.rs` | Add Cancel button; wire `CancelHandle`; handle `UpdateResult::Cancelled` in dispatch; skip history for Cancelled; update status label |

**No changes to:** `src/runner.rs`, `src/executor.rs`, `src/backends/*.rs` (any backend),
`Cargo.toml`.

---

## 10. External Research Sources

1. **Tokio docs — `tokio::sync::watch`** (Context7: `/websites/rs_tokio_1_49_0`):
   Confirmed that `watch::channel` broadcasts to multiple receivers and receivers can call
   `borrow()` to check current value without awaiting. Evaluated for Option B.

2. **Tokio docs — `tokio::sync::Mutex`** (Context7: `/websites/rs_tokio_1_49_0`):
   Confirmed async mutex is appropriate for `PrivilegedShell` (already used in the project).
   `Arc<tokio::sync::Mutex<PrivilegedShell>>` can safely be shared across tasks.

3. **`std::sync::atomic::AtomicBool` — Rust std docs**:
   `AtomicBool::swap(true, SeqCst)` provides atomic test-and-set; safe for cross-thread cancel
   signalling from the GTK main thread to Tokio worker threads.

4. **Tokio `process::Child::kill()` docs**:
   `Child::kill()` sends SIGKILL immediately. Considered for hard-killing non-root child
   processes. Not used in this implementation (soft cancel for non-root backends is safer and
   avoids partial package states). Future improvement path.

5. **GTK4-rs button sensitivity** (Context7: `/gtk-rs/gtk4-rs`):
   `widget.set_sensitive(false)` grays out and makes non-interactive.
   `widget.set_visible(false)` hides entirely. GNOME HIG recommends hiding buttons that have
   no current relevance rather than just disabling them. Cancel button is hidden (not just
   disabled) when no update is running.

6. **GNOME HIG — Destructive actions** (https://developer.gnome.org/hig/):
   "Cancel" on an in-progress operation should use `destructive-action` CSS class to signal
   that it will stop the current operation. Placement: same row as "Update All" or directly
   below it. The Cancel button uses the `destructive-action pill` CSS classes for visual
   consistency with "Update All" (`suggested-action pill`).

7. **Rust child process stdin EOF semantics** (`std::process::ChildStdin` / Tokio docs):
   Dropping `ChildStdin` closes the write end of the pipe, sending EOF to the child's stdin.
   For `sh`, this causes it to exit after processing any already-buffered commands. The
   `tokio::process::ChildStdin` has the same semantics as the std version for drop behaviour.
   This is the mechanism used by `PrivilegedShell::close()` and by our cancel path.

---

## 11. Open Questions (for Implementation Agent)

1. **Cancel during auth**: If the user cancels while the polkit dialog is shown (auth pending),
   the `PrivilegedShell::new()` will eventually return `Err("pkexec failed: authentication was
   cancelled")`. The orchestrator already handles this with `OrchestratorEvent::AuthFailed`.
   No special handling needed for cancel-during-auth; it resolves naturally.

2. **Cancel button placement**: This spec places Cancel below Update All. If a horizontal
   layout is preferred (Cancel next to Update All), the implementation agent should use an
   `gtk::Box` with `Orientation::Horizontal` wrapping both buttons.

3. **Re-checking after cancel**: After cancel, the "Update All" button is re-enabled. The
   `last_available_count()` data still reflects the pre-cancel state. The user can click
   "Update All" again or the refresh button to re-check. No special reset needed.
