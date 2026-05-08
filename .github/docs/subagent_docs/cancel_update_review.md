# Cancel Running Update — Review & QA

**Feature**: Cancel running update  
**Date**: 2026-05-08  
**Reviewer**: QA Subagent (Phase 3)  
**Spec**: `.github/docs/subagent_docs/cancel_update_spec.md`

---

## `cargo fmt --check` Result

```
Exit code: 0 — PASSED (no formatting diffs)
```

---

## Findings

### WARNING — Cancel button permanently insensitive after first use

**File**: `src/ui/window.rs`  
**Lines**: 445–452 (click handler), 539 (show on run), 712 (hide on finish)

When the user clicks the Cancel button, `btn.set_sensitive(false)` is called
immediately to prevent double-cancel. This is correct. However, `set_sensitive(true)`
is **never called** when the button is made visible again on the next update run.

Evidence:
```
grep "cancel_button.set_sensitive" src/ui/window.rs  → 0 results
```

The button starts life with GTK's default `sensitive = true`. After the user
cancels once, `sensitive` becomes `false`. The `AllFinished` / `AuthFailed`
handlers call `cancel_button.set_visible(false)` but do not reset sensitivity.
The next time the user clicks "Update All", `cancel_button.set_visible(true)` is
called at line 539 — but the button remains grayed-out and unclickable for all
future runs in the session.

**Fix required** (one line): In `update_button.connect_clicked`, immediately after
`cancel_button.set_visible(true)` at line 539, add:

```rust
cancel_button.set_sensitive(true);
```

---

### INFO-1 — `BackendError::Cancelled` is defined but never constructed

**File**: `src/backends/mod.rs` line 36

`BackendError::Cancelled` was specified and added to the enum, but no code in
the codebase ever constructs it. The orchestrator overrides errors with
`UpdateResult::Cancelled` directly, so this variant is unused.

Unlike the other unused variants (`Parse`, `Network`), it does **not** carry an
`#[allow(dead_code)]` attribute. On a Linux build this will likely produce a
`dead_code` compiler warning.

**Fix** (trivial): Add `#[allow(dead_code)]` above the variant, matching the
pattern of `Parse` and `Network`:

```rust
/// The update was cancelled by the user.
#[error("Update cancelled by user")]
#[allow(dead_code)]
Cancelled,
```

---

### INFO-2 — `unreachable!()` in history match arm is fragile

**File**: `src/ui/window.rs`, inside the `BackendFinished` handler (around line 680)

The history-recording block is guarded by `if !matches!(result, UpdateResult::Cancelled)`,
and then uses `unreachable!()` as the match arm for `Cancelled`. This is
logic-safe today but creates a latent panic path if the guard condition is ever
refactored incorrectly. The safer pattern is `_ => {}` (or an explicit
`UpdateResult::Cancelled => continue`), which is the established style in the
same file's maintenance handler where `UpdateResult::Cancelled => {}` is used.

No code change strictly required — documenting for awareness.

---

### INFO-3 — `CleanupOrchestrator` has no cancel support

**File**: `src/orchestrator.rs`

`CleanupOrchestrator::run_all` does not return a `CancelHandle` and has no
cancel checks in its backend loop. The Cancel button is not shown during
maintenance runs (it is only revealed on `update_button` click), so there is no
user-visible gap today. This was documented as a known gap in spec §2h.

---

### INFO-4 — Retry path discards `CancelHandle` silently

**File**: `src/ui/window.rs`, retry closure (around line 1000)

The retry path calls `orchestrator.run_all(event_tx)` and discards the returned
`CancelHandle`. The Cancel button correctly stays hidden during retries (it is
never set visible), and the outer `cancel_handle` slot is `None` at that point.
No functional issue; documenting for completeness.

---

## Positive Findings

All core spec requirements are correctly implemented:

| Area | Finding |
|------|---------|
| `Arc<AtomicBool>` usage | ✓ Correct — not `Mutex<bool>` |
| `cancel()` ordering | ✓ Sets flag (`swap(true)`) before spawning close task |
| Double-cancel guard | ✓ Early return on `swap` returning `true` |
| `Ordering` | ✓ `SeqCst` (conservative but correct) |
| `std::sync::Mutex` for cancel | ✓ GTK-thread safe; `tokio::sync::Mutex` only for async shell ops |
| Shell slot populated before use | ✓ Arc placed in slot in the `Ok(s)` arm before auth success event |
| Pre-loop cancel check | ✓ Emits `BackendFinished(Cancelled)` without `BackendStarted` |
| Post-run result override | ✓ Overrides any result (including `Error`) to `Cancelled` when flag set |
| No double-close | ✓ `PrivilegedShell::close()` is idempotent (`stdin.take()` + `child.wait()`) |
| Shell slot cleared after run | ✓ `guard.take()` called in post-loop cleanup |
| `set_status_cancelled()` pattern | ✓ Matches `set_status_skipped` — spinner off, `dim-label`, no retry |
| History skipped for `Cancelled` | ✓ `if !matches!(result, UpdateResult::Cancelled)` guard |
| Reboot dialog blocked | ✓ `if !has_error && !was_cancelled` gate |
| Cancel button hidden on finish | ✓ In both `AllFinished` and `AuthFailed` paths |
| Update All re-enabled | ✓ `button.set_sensitive(true)` in post-loop code |
| Accessible label on cancel btn | ✓ `Label("Cancel update")` set via `update_property` |
| No `unwrap()` on shell slot | ✓ `if let Ok(...)` / `if let Some(...)` throughout |
| `drop(runtime().spawn(...))` | ✓ Fire-and-forget task; no blocking on GTK thread |

---

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 93% | A |
| Best Practices | 90% | A- |
| Functionality | 82% | B |
| Code Quality | 91% | A- |
| Security | 97% | A+ |
| Performance | 95% | A |
| Consistency | 93% | A |
| Build Success | 90% | A- |

**Overall Grade: A- (91%)**

---

## Verdict

**NEEDS_REFINEMENT**

One WARNING-level implementation bug must be fixed before the feature ships:

> **Cancel button becomes permanently insensitive after the first cancellation.**
> Add `cancel_button.set_sensitive(true);` immediately after
> `cancel_button.set_visible(true);` in the `update_button.connect_clicked` handler
> (`src/ui/window.rs` line ~539).

Additionally, add `#[allow(dead_code)]` to `BackendError::Cancelled` in
`src/backends/mod.rs` to prevent a dead_code compiler warning on Linux builds.

All other criteria pass. The core cancel architecture is sound, safe, and
consistent with the specification.
