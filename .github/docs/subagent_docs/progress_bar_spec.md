# Spec: Determinate Progress Bar for Update Rows

**Feature:** Replace indeterminate (pulsing) progress bar with a determinate (filling) progress bar driven by a time-based linear heuristic.

**Target Files:**
- `src/ui/update_row.rs` — primary change
- `src/ui/window.rs` — remove `pulse_progress()` call

---

## 1. Current State Analysis

### Widget Setup (`src/ui/update_row.rs`)

`UpdateRow` contains a `gtk::ProgressBar` built as:

```rust
let progress_bar = gtk::ProgressBar::builder()
    .visible(false)
    .valign(gtk::Align::Center)
    .width_request(100)
    .build();
```

Neither `set_pulse_step()` nor `set_fraction()` are called at construction time, so GTK uses its default pulse step.

### Pulse trigger (`src/ui/window.rs`, lines ~340–350)

In the log-output handler future (`glib::spawn_future_local`), every incoming `(BackendKind, String)` line from the async channel calls:

```rust
row.pulse_progress();
```

Inside `UpdateRow::pulse_progress()`:

```rust
pub fn pulse_progress(&self) {
    self.progress_bar.pulse();
}
```

This places the bar in **indeterminate (pulsing) mode**: the shaded block bounces back and forth with no indication of how much work remains.

### Channel architecture

- `async_channel::Sender<(BackendKind, String)>` — carries stdout/stderr lines from all backends to the GTK main loop.
- `async_channel::Sender<(BackendKind, UpdateResult)>` — carries terminal results (`Success`, `Error`, `Skipped`).
- Lines are raw text; no structured progress percentages are emitted by any backend.

### Status lifecycle

| Method | Spinner | ProgressBar |
|---|---|---|
| `set_status_running()` | visible + spinning | visible, fraction = 0.0 |
| `set_status_success()` | hidden | hidden |
| `set_status_error()` | hidden | hidden |
| `set_status_skipped()` | hidden | hidden |
| `set_status_unknown()` | hidden | hidden |

The bar is shown only during an active update and hidden on any terminal state.

---

## 2. Problem Definition

The pulsing animation:
- Provides no information about update progress.
- Stalls visually if output pauses (e.g., during a large package download — which emits no lines).
- Conveys the same uncertainty throughout a 5-second and a 5-minute run.

The goal is to replace it with a **determinate fill** that moves continuously from 0% towards ~95%, then snaps to 100% on completion, giving the user a more useful sense of elapsed progress even when exact counts are unknown.

---

## 3. Approach Selection

Three candidate approaches were evaluated:

### Option A — Linear time-based fill ✅ SELECTED

A `glib::timeout_add_local` timer fires every **200 ms**; each tick increments the fraction by a fixed step (e.g. `0.005`), capping at **0.95**. On completion the fraction is forced to **1.0** and the timer is cancelled.

- Reaches 95% cap in `0.95 / 0.005 × 200 ms ≈ 38 seconds`.
- Always advances even during long download silences.
- No need to parse or count output lines.
- Easy to tune via a single named constant.

### Option B — Line-count heuristic

Each received line increments `fraction += STEP_PER_LINE`. Stalls completely during long silences (downloads), giving a false impression the update is stuck.

### Option C — Multi-phase fill

Fast fill to 10% on start, slow fill to 90% during output, snap to 100% on done. Combines the stall problem of Option B with extra complexity.

### Justification for Option A

Package managers emit lines in bursts (fast extraction phase) interrupted by long silences (download phase). A line-driven approach stalls during downloads, which is exactly the worst time for visible progress to freeze. A time-based approach guarantees continuous movement and better user perception of liveness.

---

## 4. GTK4 API Reference

### `gtk::ProgressBar` determinate mode

```rust
// Set fraction (0.0 – 1.0) → fills the bar proportionally
progress_bar.set_fraction(f64);

// Indeterminate mode (bouncing block) — NOT used after this change
progress_bar.pulse();
```

### `glib::timeout_add_local`

```rust
use gtk::glib;
use std::time::Duration;

let source_id: glib::SourceId = glib::timeout_add_local(
    Duration::from_millis(200),
    move || {
        // closure runs on GTK main thread every 200 ms
        glib::ControlFlow::Continue   // keep firing
        // or
        glib::ControlFlow::Break      // remove timer
    },
);

// Cancel at any time:
source_id.remove();
```

`SourceId::remove()` is safe to call from the GTK main thread (the only thread timer closures run on), so it integrates cleanly with the existing `glib::spawn_future_local` call sites.

---

## 5. Implementation Steps

### 5.1  `src/ui/update_row.rs`

#### 5.1.1  Add `glib` import

Add at the top (alongside existing imports):

```rust
use gtk::glib;
```

#### 5.1.2  Add two new fields to `UpdateRow`

```rust
#[derive(Clone)]
pub struct UpdateRow {
    pub row: adw::ExpanderRow,
    status_label: gtk::Label,
    spinner: gtk::Spinner,
    progress_bar: gtk::ProgressBar,
    pkg_rows: Rc<RefCell<Vec<adw::ActionRow>>>,
    // NEW: shared fraction state and timer handle
    progress_fraction: Rc<RefCell<f64>>,
    progress_timer: Rc<RefCell<Option<glib::SourceId>>>,
}
```

Both fields wrap their value in `Rc<RefCell<…>>` to allow the timer closure (a `'static` closure) to share mutable access with the `UpdateRow` methods.

#### 5.1.3  Initialize new fields in `UpdateRow::new()`

```rust
Self {
    row,
    status_label,
    spinner,
    progress_bar,
    pkg_rows: Rc::new(RefCell::new(Vec::new())),
    progress_fraction: Rc::new(RefCell::new(0.0)),
    progress_timer: Rc::new(RefCell::new(None)),
}
```

#### 5.1.4  Add private helper: `stop_progress_timer()`

```rust
fn stop_progress_timer(&self) {
    if let Some(id) = self.progress_timer.borrow_mut().take() {
        id.remove();
    }
}
```

#### 5.1.5  Modify `set_status_running()`

Replace the existing body with:

```rust
pub fn set_status_running(&self) {
    self.stop_progress_timer();
    *self.progress_fraction.borrow_mut() = 0.0;

    self.spinner.set_visible(true);
    self.spinner.set_spinning(true);
    self.progress_bar.set_visible(true);
    self.progress_bar.set_fraction(0.0);
    self.status_label.set_label("Updating...");
    self.status_label.set_css_classes(&["accent"]);

    // Time-based linear fill: increment by STEP every INTERVAL_MS milliseconds,
    // capping at CAP so the bar never reaches 100% before completion is confirmed.
    const INTERVAL_MS: u64 = 200;
    const STEP: f64 = 0.005;
    const CAP: f64 = 0.95;

    let fraction_rc = self.progress_fraction.clone();
    let bar = self.progress_bar.clone();
    let timer_rc = self.progress_timer.clone();

    let id = glib::timeout_add_local(
        std::time::Duration::from_millis(INTERVAL_MS),
        move || {
            let mut f = fraction_rc.borrow_mut();
            if *f < CAP {
                *f += STEP;
                bar.set_fraction(f.min(CAP));
                glib::ControlFlow::Continue
            } else {
                // Reached cap — stop firing but keep the bar visible at CAP.
                timer_rc.borrow_mut().take();
                glib::ControlFlow::Break
            }
        },
    );

    *self.progress_timer.borrow_mut() = Some(id);
}
```

**Constants rationale:**
- `INTERVAL_MS = 200` ms — smooth visually (~5 fps), low CPU overhead.
- `STEP = 0.005` — reaches 0.95 cap after ~38 seconds (190 ticks × 200 ms). Suits most package manager runs.
- `CAP = 0.95` — reserves the final 5% for the confirmed completion signal.

#### 5.1.6  Modify all completion methods to stop the timer

In each of `set_status_success`, `set_status_error`, `set_status_skipped`, `set_status_unknown`: call `self.stop_progress_timer()` before the existing `self.progress_bar.set_visible(false)` line, and set fraction to `1.0` immediately before hiding:

```rust
pub fn set_status_success(&self, count: usize) {
    self.stop_progress_timer();
    self.progress_bar.set_fraction(1.0);   // flash 100% before hiding
    self.spinner.set_visible(false);
    self.spinner.set_spinning(false);
    self.progress_bar.set_visible(false);
    let msg = if count == 0 {
        "Up to date".to_string()
    } else {
        format!("{count} updated")
    };
    self.status_label.set_label(&msg);
    self.status_label.set_css_classes(&["success"]);
}

pub fn set_status_error(&self, msg: &str) {
    self.stop_progress_timer();
    self.progress_bar.set_fraction(1.0);
    self.spinner.set_visible(false);
    self.spinner.set_spinning(false);
    self.progress_bar.set_visible(false);
    self.status_label.set_label(&format!("Error: {}", msg));
    self.status_label.set_css_classes(&["error"]);
}

pub fn set_status_skipped(&self, msg: &str) {
    self.stop_progress_timer();
    self.progress_bar.set_fraction(1.0);
    self.spinner.set_visible(false);
    self.spinner.set_spinning(false);
    self.progress_bar.set_visible(false);
    self.status_label.set_label(msg);
    self.status_label.set_css_classes(&["dim-label"]);
}

pub fn set_status_unknown(&self, msg: &str) {
    self.stop_progress_timer();
    self.progress_bar.set_fraction(1.0);
    self.spinner.set_visible(false);
    self.spinner.set_spinning(false);
    self.progress_bar.set_visible(false);
    self.status_label.set_label(msg);
    self.status_label.set_css_classes(&["dim-label"]);
}
```

#### 5.1.7  Remove (or keep inert) `pulse_progress()`

The method is called by `window.rs`. To avoid a compile error while the window.rs change is made in the same commit, either:

- **Delete** the method and update `window.rs` simultaneously, OR
- Temporarily make it a no-op.

The preferred approach is to delete it and update `window.rs` in the same change.

---

### 5.2  `src/ui/window.rs`

In the log-output handler future (approximately line 340):

**Remove** the line:

```rust
row.pulse_progress();
```

The surrounding context is:

```rust
glib::spawn_future_local(async move {
    while let Ok((kind, line)) = rx.recv().await {
        log_ref2.append_line(&format!("[{kind}] {line}"));
        let borrowed = rows_for_log.borrow();
        if let Some((_, row)) = borrowed.iter().find(|(k, _)| *k == kind) {
            row.pulse_progress();   // ← DELETE this line
        }
    }
});
```

After deletion, the `if let Some(...)` block body is empty. Remove the entire `if let` block:

```rust
glib::spawn_future_local(async move {
    while let Ok((kind, line)) = rx.recv().await {
        log_ref2.append_line(&format!("[{kind}] {line}"));
    }
});
```

---

## 6. New Structs / Fields / Variants

No new message variants or structs are required. Only two new fields are added to `UpdateRow`:

| Field | Type | Purpose |
|---|---|---|
| `progress_fraction` | `Rc<RefCell<f64>>` | Shared mutable current fraction value |
| `progress_timer` | `Rc<RefCell<Option<glib::SourceId>>>` | Handle for the active `timeout_add_local` source |

---

## 7. Files Changed Summary

| File | Change |
|---|---|
| `src/ui/update_row.rs` | Add 2 fields; add `stop_progress_timer()`; rewrite `set_status_running()`; update 4 completion methods; remove `pulse_progress()` |
| `src/ui/window.rs` | Remove `row.pulse_progress()` call and empty `if let` block |

---

## 8. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| **Timer outlives backend run** — if a future is dropped before completion, the timer keeps firing. | `stop_progress_timer()` is called in all 4 terminal-state methods. The bar is hidden but the timer continuing is harmless (the bar is not visible). |
| **Multiple concurrent updates** — if `set_status_running()` is called twice (re-trigger), two timers could run. | `stop_progress_timer()` is called at the top of `set_status_running()` before starting a new timer, preventing accumulation. |
| **`glib::SourceId` misuse on non-GTK thread** | All timer calls are inside `glib::spawn_future_local` / GTK main thread. No cross-thread risk. |
| **38-second default too slow for quick backends** | The bar still reaches a meaningfully filled state (e.g., 0.15 = 15% for a 6-second run) before snapping to 100% on completion. Users see motion, not a stuck bar. The `STEP` constant can be tuned. |
| **`Clone` on `UpdateRow` with `Rc<RefCell<…>>`** | `Rc` is already used for `pkg_rows`. The new fields follow the same pattern — clones share the same underlying state, which is correct for the timer to remain in sync with all clones. |

---

## 9. Non-Functional Considerations

- **CPU overhead:** `glib::timeout_add_local` with a 200 ms interval executes ~5 times/second per active row. Each tick is a single float add and a `set_fraction()` call — negligible.
- **Thread safety:** All state (`Rc<RefCell<…>>`) is confined to the GTK main thread. No `Arc` or `Mutex` required.
- **Accessibility:** A determinate progress bar has a meaningful ARIA `value` that screen readers can report. This is an improvement over indeterminate mode, which only signals "busy".
