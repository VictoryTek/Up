# Specification: Update Progress Bar

**Feature:** Functional filling progress bar under the "Updating…" label during active update runs  
**Project:** Up (GTK4/libadwaita system updater, Rust)  
**Spec Path:** `.github/docs/subagent_docs/update_progress_bar_spec.md`

---

## 1. Current State Analysis

### 1.1 Status Label (The "Updating" Label)

In `src/ui/window.rs`, inside `build_update_page()`, a `gtk::Label` named `status_label` is
created and appended to `content_box`:

```rust
let status_label = gtk::Label::builder()
    .label("Detect available updates across your system.")
    .css_classes(vec!["dim-label"])
    .build();
content_box.append(&status_label);
```

This label transitions through the following states during an update run:
- `"Detect available updates across your system."` — initial idle state
- `"Checking for updates..."` — while `run_checks` runs
- `"N updates available"` — after check completes
- `"Authenticating…"` — when `OrchestratorEvent::AuthStarted` is received
- `"Updating…"` — when `OrchestratorEvent::AuthSucceeded` is received
- `"Update complete."` / `"Update completed with errors."` — after `AllFinished`

**There is currently no visual progress indicator** (no bar, spinner row, or other widget)
adjacent to the "Updating…" label that would show how far through the update run the app is.

### 1.2 Widget Hierarchy (Update Page)

`build_update_page()` constructs:
```
page_box (gtk::Box, Vertical)
├── restart_banner (adw::Banner)          ← prepended later
├── metered_banner (adw::Banner)          ← appended later
└── scrolled (gtk::ScrolledWindow)
    └── clamp (adw::Clamp, max 600px)
        └── content_box (gtk::Box, Vertical, spacing=18)
            ├── status_label (gtk::Label)         ← shows "Updating…" text
            ├── sys_info_group (adw::PreferencesGroup)
            ├── backends_group (adw::PreferencesGroup)
            ├── log_panel.expander (gtk::Expander)
            └── update_button (gtk::Button "Update All")
```

### 1.3 Event Flow

The `UpdateOrchestrator::run_all()` in `src/orchestrator.rs` runs backends sequentially and
sends `OrchestratorEvent` messages through an `async_channel`. The GTK main thread receives
these in the `glib::spawn_future_local` block inside the button click handler:

```
AuthStarted → AuthSucceeded → (BackendStarted → BackendLog* → BackendFinished)* → AllFinished
```

Backends are run **sequentially** (one at a time). The number of active backends is known
before the orchestrator starts: it equals `backends.len()` in the local `backends` variable
collected just before spawning.

### 1.4 Update Count Tracking

The `total_available` and `pending_checks` `Rc<RefCell<usize>>` cells in `build_update_page()`
track available update counts for the check phase. The update phase uses a separate `backends`
`Vec` built from the `detected` list filtered by skip status.

### 1.5 Existing Dependencies

`Cargo.toml` already includes:
- `gtk = { version = "0.9", package = "gtk4", features = ["v4_12"] }` — `gtk::ProgressBar` is
  available with no new dependencies needed.
- `adw = { version = "0.7", package = "libadwaita", features = ["v1_5"] }` — no adw equivalent
  for a simple progress bar is needed; `gtk::ProgressBar` is the correct widget.

---

## 2. Problem Definition

When "Update All" is pressed, the user sees the `status_label` switch to "Updating…" but has
no visual indication of:
- Whether the update has started (versus still authenticating)
- How far through the multi-backend update sequence the app is
- That anything is happening (the per-row spinners are in the backend list, which may be
  off-screen on smaller windows)

A **filling progress bar** placed directly below the status label provides clear, at-a-glance
feedback without requiring the user to scroll to see individual backend rows.

---

## 3. Proposed Solution Architecture

### 3.1 Widget: `gtk::ProgressBar`

Use **`gtk::ProgressBar`** (not `adw::LevelBar`, not a custom widget).

**Rationale:**
- Available in gtk4-rs v0.9 with no new dependencies
- `set_fraction(f64)` provides deterministic 0.0–1.0 progress
- `pulse()` provides indeterminate activity during authentication
- Native GTK4 widget, styled correctly with the current theme
- Fits naturally in the vertical `content_box` layout

**API used (gtk4-rs v0.9 / GTK 4.12):**
```rust
let progress_bar = gtk::ProgressBar::new();
progress_bar.set_fraction(0.0_f64);   // set deterministic progress
progress_bar.pulse();                  // indeterminate bounce
progress_bar.set_visible(false);      // hide when idle
```

### 3.2 Progress Mode: Determinate (Fraction-Based)

Use **fraction-based** (`set_fraction`) progress — not pulse-only mode.

**Rationale:**
- The number of active backends is known before the orchestrator starts
- `BackendFinished` events arrive once per backend, deterministically
- Fraction = `finished_count / total_backends` gives accurate, honest progress
- For N backends: the bar fills in N equal steps as backends complete

**For the authentication phase** (between button click and `AuthSucceeded`): start the
progress bar at 0.0 and pulse it so the user sees activity without false precision.
Once `AuthSucceeded` fires, switch to fraction mode starting at 0.0 and advance on each
`BackendFinished`.

Actually, for simplicity and correctness, **pure fraction mode** is used:
- On update start: visible=true, fraction=0.0 (bar visible but empty = in progress)
- On each `BackendFinished`: fraction = finished / total
- On `AllFinished`: fraction = 1.0, then hide after a 1.5-second delay

The empty-but-visible bar during auth already communicates "something has started."
Per-row spinners on the `UpdateRow` widgets give sub-backend activity feedback.

### 3.3 Placement in Widget Tree

The progress bar is inserted **between `status_label` and `sys_info_group`** inside
`content_box`:

```
content_box (gtk::Box, Vertical, spacing=18)
├── status_label (gtk::Label)
├── progress_bar (gtk::ProgressBar)     ← NEW, hidden when idle
├── sys_info_group (adw::PreferencesGroup)
├── backends_group (adw::PreferencesGroup)
├── log_panel.expander (gtk::Expander)
└── update_button (gtk::Button)
```

This placement puts the bar directly under the status text, forming a natural two-line
status area at the top of the scrollable content area.

### 3.4 Styling

Apply the `"osd"` CSS class to the `ProgressBar` for a visually prominent appearance
consistent with modern GNOME/libadwaita applications:

```rust
let progress_bar = gtk::ProgressBar::builder()
    .visible(false)
    .css_classes(vec!["osd"])
    .build();
```

The `osd` class gives the bar a rounded pill appearance that fits well within the
`adw::Clamp`-constrained content area.

### 3.5 State Variables Needed

Two new `Rc<Cell<usize>>` variables track progress within the update handler scope:

| Variable | Type | Purpose |
|---|---|---|
| `progress_total` | `Rc<Cell<usize>>` | Total active (non-skipped) backends for this run |
| `progress_done` | `Rc<Cell<usize>>` | Count of `BackendFinished` events received so far |

These are reset at the start of each new update run.

### 3.6 Lifecycle

| Event | Action |
|---|---|
| "Update All" clicked | `progress_done.set(0)`, `progress_total.set(backends.len())`, `progress_bar.set_fraction(0.0)`, `progress_bar.set_visible(true)` |
| `AuthStarted` | (already showing at 0.0 — no change) |
| `AuthSucceeded` | (no change — bar already visible at 0.0) |
| `AuthFailed(e)` | `progress_bar.set_visible(false)` |
| `BackendStarted(kind)` | (no change to bar — row spinner handles per-backend activity) |
| `BackendFinished(kind, result)` | `progress_done += 1`, `progress_bar.set_fraction(done as f64 / total as f64)` |
| `AllFinished` | `progress_bar.set_fraction(1.0)`, schedule hide via `glib::timeout_add_local_once(Duration::from_millis(1500), ...)` |

The `glib::timeout_add_local_once` delay lets the user see the completed (full) bar briefly
before it disappears:
```rust
let weak_bar = progress_bar.downgrade();
glib::timeout_add_local_once(Duration::from_millis(1500), move || {
    if let Some(bar) = weak_bar.upgrade() {
        bar.set_visible(false);
    }
});
```

### 3.7 Retry Path

The retry handler (per-backend retry button in `UpdateRow`) also spawns an orchestrator and
processes events in a separate `glib::spawn_future_local`. It currently does **not** show a
progress bar. The progress bar is scoped to the "Update All" flow only, for simplicity.
The retry path is single-backend, so the per-row `Updating...` status label and spinner
already give sufficient feedback.

---

## 4. Data Flow Design

```
[GTK Main Thread]                     [Background Thread]
                                       UpdateOrchestrator
                                       ┌──────────────────┐
update button clicked ───────────────► │ run_all(tx)      │
set fraction=0.0                       │                  │
                                       │ AuthStarted ────►│──► tx.send()
receive AuthStarted                    │                  │
                                       │ AuthSucceeded ──►│──► tx.send()
receive AuthSucceeded                  │                  │
set status "Updating…"                 │ BackendStarted ─►│──► tx.send()
                                       │ BackendLog ─────►│──► tx.send()
receive BackendFinished                │ BackendFinished ►│──► tx.send()
  done += 1                            │                  │
  bar.set_fraction(done/total)         │ AllFinished ────►│──► tx.send()
                                       └──────────────────┘
receive AllFinished
  bar.set_fraction(1.0)
  schedule_hide(1500ms)
```

The `async_channel` (`event_rx.recv().await`) already bridges the background thread to the
GTK main loop via `glib::spawn_future_local`. No new channels or threads are required.

---

## 5. Implementation Steps

### Step 1: Create the ProgressBar widget

In `src/ui/window.rs`, inside `build_update_page()`, immediately after the `status_label`
creation and `content_box.append(&status_label)` call:

```rust
let progress_bar = gtk::ProgressBar::builder()
    .visible(false)
    .css_classes(vec!["osd"])
    .build();
content_box.append(&progress_bar);
```

### Step 2: Add state variables inside `update_button.connect_clicked`

At the top of the button click handler closure body, before the orchestrator is constructed:

```rust
let progress_total: Rc<Cell<usize>> = Rc::new(Cell::new(0));
let progress_done: Rc<Cell<usize>> = Rc::new(Cell::new(0));
```

These must be captured by `glib::spawn_future_local` inside the closure, so they need to be
in scope at the closure definition and captured as `#[strong]`.

### Step 3: Wire progress bar into the click handler

Inside `update_button.connect_clicked(glib::clone!(..., move |button| { ... }))`, after
the `button.set_sensitive(false)` and `updating.set(true)` calls, but before the backends
vec is built:

```rust
// Reset progress state
progress_done.set(0);
```

After the `backends` vec is built (after the `.collect()` call), record total:

```rust
progress_total.set(backends.len());
progress_bar.set_fraction(0.0);
progress_bar.set_visible(true);
```

### Step 4: Add `progress_bar`, `progress_done`, `progress_total` to the `glib::clone!` captures

In the `glib::spawn_future_local(glib::clone!(...` block, add:
```rust
#[weak] progress_bar,
#[strong] progress_done,
#[strong] progress_total,
```

### Step 5: Handle `BackendFinished` event

Inside the `while let Ok(event) = event_rx.recv().await { match event { ... } }` loop, in
the `OrchestratorEvent::BackendFinished(kind, result)` arm, **after** the existing row status
update, add:

```rust
let done = progress_done.get() + 1;
progress_done.set(done);
let total = progress_total.get();
if total > 0 {
    progress_bar.set_fraction(done as f64 / total as f64);
}
```

### Step 6: Handle `AllFinished` event

In the `OrchestratorEvent::AllFinished` arm (currently just `break`), change to:

```rust
OrchestratorEvent::AllFinished => {
    progress_bar.set_fraction(1.0);
    let weak_bar = progress_bar.downgrade();
    glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
        if let Some(bar) = weak_bar.upgrade() {
            bar.set_visible(false);
        }
    });
    break;
}
```

### Step 7: Handle `AuthFailed` event

In the `OrchestratorEvent::AuthFailed(e)` arm, after the existing button re-enable, add:

```rust
progress_bar.set_visible(false);
```

### Step 8: Reset on next check cycle

The progress bar is already hidden after `AllFinished`. On each new "Update All" click,
the handler sets `visible(true)` and `fraction(0.0)` again (Step 3). No additional reset
logic is needed.

---

## 6. Files Modified

| File | Change |
|---|---|
| `src/ui/window.rs` | Only file changed — create `progress_bar` widget, add state cells, wire lifecycle |

No other files require changes. The `Backend` trait, `OrchestratorEvent` enum,
`UpdateOrchestrator`, `UpdateRow`, and all backend implementations remain unchanged.

---

## 7. New Struct Fields / Signals

None. All new state is local to `build_update_page()` and its closures. No new public API
is introduced.

---

## 8. Risks and Mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| `progress_bar` weak reference dropped before timeout fires | Very Low | The `content_box` holds a strong reference to the widget; the weak upgrade will always succeed unless the window is destroyed during the 1500ms window |
| Single-backend case — bar jumps 0→100% | Low | This is accurate and expected; the per-row spinner communicates sub-backend activity. A pulse during single-backend auth would require more complexity for marginal UX gain |
| NixOS rebuild takes 30+ minutes — bar stuck at 0% for a long time | Medium | For NixOS, there is only one backend (NixBackend), so the bar stays at 0% until the rebuild completes. This is honest. A future enhancement could add pulse mode during extended single-backend runs. Out of scope for this feature. |
| `progress_total` is 0 when all backends are skipped | Low | The "Update All" button is only sensitive when `non_skipped_total > 0`, so this cannot happen. The guard `if total > 0` in Step 5 is a belt-and-suspenders safety check. |
| Retry path also shows wrong state | Not Applicable | Retry path does not use the progress bar (see §3.7). |
| CSS class `"osd"` not available in GTK 4.12 | None | `osd` is a standard GTK CSS class present since GTK 3. Available on all supported GTK 4 versions. |

---

## 9. Dependencies

No new Cargo dependencies required. `gtk::ProgressBar` and `glib::timeout_add_local_once`
are both already available via the existing `gtk4 v0.9` and `glib v0.20` dependencies.

**Context7 verification:** `/gtk-rs/gtk4-rs` was queried. The gtk4-rs v0.9 crate wraps GTK 4.12
and provides `gtk::ProgressBar` with `.set_fraction(f64)`, `.set_visible(bool)`, `.pulse()`,
and the builder pattern. `gtk::ProgressBar::builder().css_classes(...).build()` is the
idiomatic construction pattern. No deprecated APIs are used.

---

## 10. Acceptance Criteria

1. When "Update All" is pressed, a progress bar becomes visible below the "Updating…" label.
2. The bar starts empty (fraction = 0.0) when the update begins.
3. After each backend finishes, the bar advances by `1 / N` (where N = active backend count).
4. After all backends finish, the bar fills to 100% and disappears after ~1.5 seconds.
5. If authentication is cancelled, the bar disappears immediately.
6. The bar is not visible in idle state, after check runs, or after the update completes.
7. `cargo build` compiles without errors or warnings.
8. `cargo clippy -- -D warnings` passes.
9. `cargo fmt --check` passes.
