# Review: Determinate Progress Bar Feature

**Reviewed:** `src/ui/update_row.rs`, `src/ui/window.rs`  
**Spec:** `.github/docs/subagent_docs/progress_bar_spec.md`  
**Date:** 2026-04-19

---

## Build Results

| Check | Result |
|---|---|
| `cargo build` | ✅ PASS |
| `cargo clippy -- -D warnings` | ✅ PASS |
| `cargo fmt --check` | ✅ PASS |
| `cargo test` (12 tests) | ✅ PASS |

---

## Spec Compliance Findings

### ✅ `pulse_progress()` removed

Confirmed absent from both `src/ui/update_row.rs` and `src/ui/window.rs`. The log handler in `window.rs` was correctly reduced to only the `log_ref2.append_line(...)` call, with the surrounding `if let Some(...)` block fully removed.

---

### ✅ Two new fields added to `UpdateRow`

Both fields are present and initialized in `new()`:

```rust
progress_timer: Rc<RefCell<Option<glib::SourceId>>>,
progress_fraction: Rc<Cell<f64>>,
```

**Note:** The implementation uses `Rc<Cell<f64>>` instead of the spec's `Rc<RefCell<f64>>`. `Cell<T>` is the correct choice for `Copy` types — it avoids the runtime borrow-check overhead of `RefCell` and cannot panic. This is a deliberate, correct improvement over the spec.

---

### ✅ `set_status_running()` — timer lifecycle

- Previous timer is cancelled before starting a new one (guard against double-start) ✅
- `progress_fraction` reset to `0.0` ✅
- `progress_bar.set_fraction(0.0)` set at entry ✅
- `glib::timeout_add_local(200ms)` used correctly ✅
- Constants: `INTERVAL_MS = 200`, `STEP = 0.005`, `CAP = 0.95` — match spec ✅
- New `SourceId` stored in `self.progress_timer` ✅

**Minor deviation:** The timer closure always returns `ControlFlow::Continue` and caps the value via `.min(0.95)`, rather than returning `ControlFlow::Break` and self-clearing the timer handle as the spec describes. The net effect is:

- The timer continues firing every 200 ms after reaching the 0.95 cap (setting `0.95` redundantly ~5x/second).
- There is no double-remove risk: `stop_progress_timer()` always holds the live `SourceId` and cancels it cleanly.

This is marginally less efficient than a self-terminating timer but is **not a bug**. The simplicity benefit (no `timer_rc` cloned into the closure) outweighs the negligible CPU cost.

---

### ✅ `stop_progress_timer()` private helper

Implementation:

```rust
fn stop_progress_timer(&self) {
    if let Some(source_id) = self.progress_timer.borrow_mut().take() {
        source_id.remove();
    }
    self.progress_fraction.set(1.0);
    self.progress_bar.set_fraction(1.0);
}
```

The spec placed `progress_bar.set_fraction(1.0)` in each completion method individually. The implementation consolidates it into the helper — a valid DRY improvement. The fraction and bar update are always applied on completion regardless of path, which is correct.

---

### ✅ All completion methods call `stop_progress_timer()`

| Method | Calls `stop_progress_timer()` first | Timer cancelled | Bar hidden |
|---|---|---|---|
| `set_status_success()` | ✅ | ✅ | ✅ |
| `set_status_error()` | ✅ | ✅ | ✅ |
| `set_status_skipped()` | ✅ | ✅ | ✅ |
| `set_status_unknown()` | ✅ | ✅ | ✅ |

---

### ✅ `set_fraction(0.0..=1.0)` capping

- During run: fraction is bounded to `0.95` via `.min(0.95)` ✅
- On completion: `stop_progress_timer()` sets `1.0` before the bar is hidden ✅
- No value can exceed 1.0 (GTK clamps, but implementation also prevents it) ✅

---

### ✅ Memory safety — `Rc` usage in closures

The timer closure captures two `Rc` clones:

```rust
let progress_bar = self.progress_bar.clone();  // gtk::ProgressBar (GObject ref)
let fraction_rc = self.progress_fraction.clone();  // Rc<Cell<f64>>
```

- `gtk::ProgressBar` is a reference-counted GObject — cloning increments the ref count safely ✅
- `Rc<Cell<f64>>` is confined to the GTK main thread — the closure is a `glib::timeout_add_local` callback, which also runs only on the main thread ✅
- `timer_rc` is **not** captured inside the closure (unlike the spec) — this eliminates any borrow-inside-closure risk ✅
- No use-after-free risk: the timer fires on the GTK main thread; `SourceId::remove()` is called from the same thread in completion methods ✅

---

### ✅ No orphaned timers on re-run

If `set_status_running()` is called while a timer is already active (e.g., "Update All" clicked twice):

```rust
// Cancel any previously running timer before starting a new one.
if let Some(source_id) = self.progress_timer.borrow_mut().take() {
    source_id.remove();
}
```

Properly cancels the in-flight source before registering a new one.

---

### ✅ No unused imports, no dead code

- `glib` was already imported (`use gtk::glib;`) ✅
- `std::time::Duration` already imported ✅
- `std::cell::Cell` added to existing `use std::cell::{Cell, RefCell};` ✅
- Clippy produces zero warnings ✅

---

## Score Table

| Category | Score | Grade |
|---|---|---|
| Specification Compliance | 92% | A |
| Best Practices | 97% | A+ |
| Functionality | 100% | A+ |
| Code Quality | 96% | A+ |
| Security | 100% | A+ |
| Performance | 93% | A |
| Consistency | 97% | A+ |
| Build Success | 100% | A+ |

**Overall Grade: A+ (97%)**

---

## Summary

The implementation correctly replaces the indeterminate pulsing progress bar with a time-based determinate fill. All required changes are present:

- `pulse_progress()` is removed from both files
- Timer starts in `set_status_running()`, guarded against double-start
- All four terminal-state methods cancel the timer via the `stop_progress_timer()` helper
- `Cell<f64>` (an improvement over the spec's `RefCell<f64>`) eliminates borrow-panic risk
- Timer closure is clean — no self-referential borrow; no double-remove risk
- Fraction caps at 0.95 during run and completes at 1.0 on any terminal state
- All four build checks pass with zero warnings

There is one minor efficiency deviation (timer does not self-terminate at the 0.95 cap), but it introduces no bugs and carries negligible cost.

---

## Verdict

**PASS**
