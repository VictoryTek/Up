# Specification: LogPanel Performance Improvements (Backlog Item 7)

**Feature Name:** `logpanel_perf`  
**Date:** 2026-05-07  
**Scope:** `src/ui/log_panel.rs`, `src/ui/update_row.rs`  
**Status:** DRAFT

---

## 1. Current State Analysis

### 1.1 `src/ui/log_panel.rs`

```
LogPanel {
    expander: gtk::Expander,
    text_view: gtk::TextView,
    scroll_mark: gtk::TextMark,
}
```

**`append_line()` — exact current implementation:**

```rust
pub fn append_line(&self, line: &str) {
    let clean = strip_ansi(line);
    let buffer = self.text_view.buffer();
    let mut end = buffer.end_iter();
    buffer.insert(&mut end, &clean);
    buffer.insert(&mut end, "\n");

    // Auto-scroll to bottom
    buffer.move_mark(&self.scroll_mark, &buffer.end_iter());
    self.text_view.scroll_mark_onscreen(&self.scroll_mark);
}
```

**Problems:**

1. **No line cap.** `buffer.insert()` is called for every log line without ever trimming the top. During a full system update across several package managers (APT + Flatpak + Nix), thousands of lines can accumulate. GTK `TextBuffer` internally holds the entire text as a gap buffer; very large buffers slow down rendering and increase memory.

2. **`scroll_mark_onscreen` called synchronously on every line.** This forces a GTK layout/redraw pass per-line. When lines arrive faster than 60 fps (common for `apt upgrade` verbose output), this degrades UI frame rate visibly. The scroll target does not change between two adjacent calls — the work is redundant until the frame is actually painted.

### 1.2 `src/ui/update_row.rs`

```
UpdateRow {
    row: adw::ExpanderRow,
    status_label: gtk::Label,
    spinner: gtk::Spinner,
    progress_bar: gtk::ProgressBar,          // ← fake
    pkg_rows: Rc<RefCell<Vec<adw::ActionRow>>>,
    progress_timer: Rc<RefCell<Option<glib::SourceId>>>,  // ← fake
    progress_fraction: Rc<Cell<f64>>,        // ← fake
}
```

**`set_status_running()` — exact current implementation:**

```rust
pub fn set_status_running(&self) {
    if let Some(source_id) = self.progress_timer.borrow_mut().take() {
        source_id.remove();
    }
    self.progress_fraction.set(0.0);
    self.spinner.set_visible(true);
    self.spinner.set_spinning(true);
    self.progress_bar.set_visible(true);
    self.progress_bar.set_fraction(0.0);
    self.status_label.set_label("Updating...");
    self.status_label.set_css_classes(&["accent"]);

    let progress_bar = self.progress_bar.clone();
    let fraction_rc = self.progress_fraction.clone();

    let source_id = glib::timeout_add_local(Duration::from_millis(200), move || {
        let current = fraction_rc.get();
        let new_val = (current + 0.005).min(0.95);
        fraction_rc.set(new_val);
        progress_bar.set_fraction(new_val);
        glib::ControlFlow::Continue
    });

    *self.progress_timer.borrow_mut() = Some(source_id);
}
```

**`stop_progress_timer()` — cleanup helper:**

```rust
fn stop_progress_timer(&self) {
    if let Some(source_id) = self.progress_timer.borrow_mut().take() {
        source_id.remove();
    }
    self.progress_fraction.set(1.0);
    self.progress_bar.set_fraction(1.0);
}
```

**Problems:**

1. **The `ProgressBar` is fake.** It never reaches 1.0 (caps at 0.95) because the real progress is unknown. This is acknowledged by design but the result is misleading UX — it implies deterministic progress where none exists.

2. **A `glib::timeout_add_local` repeating timer fires every 200 ms** solely to advance the fake fraction. This is unnecessary GLib event loop pressure. If the timer is not properly cancelled (edge case: early return on auth failure before `stop_progress_timer` is reached), it leaks a repeating GLib source.

3. **`gtk::Spinner` already exists in the row** and is shown/hidden correctly for Checking state. It is also shown during Running, making both widgets visible simultaneously — redundant visual noise.

4. **`adw::Spinner`** (libadwaita's dedicated spinner widget added in libadwaita 1.6) is NOT available because `Cargo.toml` specifies `features = ["v1_5"]`. The existing `gtk::Spinner` is the correct widget to keep.

### 1.3 `src/ui/mod.rs`

No changes needed. Only declares the sub-modules.

### 1.4 `src/backends/mod.rs`

No changes needed. `Backend` trait is unchanged.

---

## 2. Research Summary

### Source 1 — GTK4 TextBuffer deletion API (gtk-rs/gtk4-rs docs)

`TextBufferExt` provides:
- `buffer.line_count() -> i32` — total number of lines (always ≥ 1; empty buffer returns 1)
- `buffer.start_iter() -> TextIter` — iterator at offset 0
- `buffer.iter_at_line(line: i32) -> Option<TextIter>` — iterator at start of given line; returns `None` if line is out of range
- `buffer.delete(start: &mut TextIter, end: &mut TextIter)` — deletes text in `[start, end)` in-place; iterators are updated to point to the merge point

The canonical pattern for FIFO-evicting the first N lines:

```rust
let mut start = buffer.start_iter();
if let Some(mut end) = buffer.iter_at_line(n_to_delete) {
    buffer.delete(&mut start, &mut end);
}
```

**Source:** gtk4-rs TextBuffer trait documentation; GNOME GTK4 C reference `gtk_text_buffer_delete_interactive` / `gtk_text_buffer_get_iter_at_line`.

### Source 2 — `glib::timeout_add_local_once` for one-shot deferred callbacks

Confirmed signature (gtk-rs-core 0.20):

```rust
pub fn timeout_add_local_once<F>(interval: Duration, func: F)
where
    F: FnOnce() + 'static,
```

Returns nothing (unlike `timeout_add_local` which returns `SourceId`). For debounce cancellation without a return value, use `timeout_add_local` with `ControlFlow::Break` on first invocation, or simply use the `Rc<Cell<bool>>` guard pattern to skip duplicate scheduling (preferred — no cancellation needed).

**Source:** gtk-rs-core source (`glib/src/source.rs`) and Context7 glib documentation.

### Source 3 — `Rc<Cell<bool>>` debounce guard on the GTK main thread

All GTK callbacks execute on the main thread. `Rc<Cell<bool>>` is the idiomatic single-threaded flag type — no `Arc`, no `Mutex`, no `RefCell` overhead. Pattern:

```rust
let pending = Rc::new(Cell::new(false));

// Inside append_line:
if !pending.get() {
    pending.set(true);
    let pending_clone = pending.clone();
    let tv = self.text_view.downgrade();    // WeakRef
    glib::timeout_add_local_once(Duration::from_millis(80), move || {
        pending_clone.set(false);
        if let Some(view) = tv.upgrade() {
            // perform the scroll
        }
    });
}
```

**Source:** gtk-rs-core `clone!` macro docs; Rust `std::cell::Cell` documentation; GTK4 main-thread model documentation.

### Source 4 — `glib::WeakRef` for safe widget captures in deferred callbacks

`gtk::prelude::ObjectExt::downgrade()` returns a `glib::WeakRef<T>` that does not keep the widget alive. If the widget is destroyed before the timeout fires, `upgrade()` returns `None` and the callback becomes a no-op. This is the required pattern for any timeout callback that captures a UI widget, to avoid extending widget lifetime or causing use-after-free semantics.

```rust
let weak_view: glib::WeakRef<gtk::TextView> = self.text_view.downgrade();
glib::timeout_add_local_once(Duration::from_millis(80), move || {
    if let Some(view) = weak_view.upgrade() {
        let mark = view.buffer().mark("scroll-end").unwrap();
        view.scroll_mark_onscreen(&mark);
    }
});
```

**Source:** gtk-rs-core `WeakRef<T>` documentation; GNOME HIG pattern for closures referencing widgets.

### Source 5 — `gtk::Spinner` vs `adw::Spinner`

`gtk::Spinner` (GTK4): present since GTK 4.0. Methods:
- `spinner.set_spinning(bool)` — starts/stops the animation
- `spinner.set_visible(bool)` — shows/hides the widget

`adw::Spinner` (libadwaita): added in libadwaita **1.6** behind the `v1_6` feature. The current `Cargo.toml` specifies `features = ["v1_5"]`. `adw::Spinner` is therefore NOT available and must not be used. The `gtk::Spinner` already used in `update_row.rs` is correct.

**Important:** When `gtk::Spinner` is hidden (`set_visible(false)`), it also stops animating — so calling `set_spinning(false)` before `set_visible(false)` is good practice but not strictly required. The animation does not waste GPU cycles while the widget is not visible.

**Source:** libadwaita changelog; GTK4 `GtkSpinner` C docs; libadwaita-rs 0.7 feature flags.

### Source 6 — GTK4 ProgressBar indeterminate mode vs Spinner

`gtk::ProgressBar` has two modes:
- **Determinate:** `set_fraction(f64)` — shows a filled bar from 0.0 to 1.0
- **Indeterminate (pulsing):** `pulse()` — advances a bouncing block; requires a timer to call `pulse()` repeatedly

The current code uses the *determinate mode* with a fake crawling fraction — worse than either native mode. The libadwaita design guidelines explicitly recommend `GtkSpinner` for "in-progress, unknown duration" states, while a `ProgressBar` in pulsing mode is acceptable but more visually heavy. Given that `gtk::Spinner` is already present and working in the row, removing `ProgressBar` is the correct simplification.

**Source:** GNOME HIG "Progress & loading" pattern; GTK4 `GtkProgressBar` API reference; libadwaita widget gallery.

### Source 7 — Batch deletion amortisation for TextBuffer line eviction

GTK `TextBuffer` stores text in a gap buffer. Calling `buffer.delete()` for a range is O(text_length_in_range + tail_shift). The operation cost is proportional to bytes deleted plus bytes moved. Deleting a single line at a time (called once per appended line whenever `line_count > 5000`) would cause one extra O(N) deletion per insertion — acceptable at 5000 lines but wasteful.

Preferred: delete a batch of 100+ lines whenever the cap is exceeded. This amortises the cost across 100 subsequent insertions. The batch size should be chosen to be large enough to amortise but small enough that the log does not appear to "jump" when eviction occurs. 100 lines (2%) of a 5000-line cap is the recommended value.

**Source:** GTK `GtkTextBuffer` internals documentation; practical benchmark data from terminal emulator implementations (e.g., VTE's ring buffer design).

---

## 3. Proposed Changes

### A. LogPanel Buffer Cap (FIFO Eviction)

**File:** `src/ui/log_panel.rs`

#### Constants to add

```rust
/// Maximum number of lines retained in the log panel buffer.
const LINE_CAP: i32 = 5_000;
/// Number of lines to evict from the top when the cap is exceeded.
const EVICT_BATCH: i32 = 100;
```

#### Struct changes

The `LogPanel` struct gains one new field for the debounce flag (see Section B). No new fields for the line cap — it is a pure post-insert operation.

#### `append_line` — new implementation

```rust
pub fn append_line(&self, line: &str) {
    let clean = strip_ansi(line);
    let buffer = self.text_view.buffer();
    let mut end = buffer.end_iter();
    buffer.insert(&mut end, &clean);
    buffer.insert(&mut end, "\n");

    // --- A. FIFO eviction ---
    if buffer.line_count() > LINE_CAP {
        let mut start = buffer.start_iter();
        if let Some(mut evict_end) = buffer.iter_at_line(EVICT_BATCH) {
            buffer.delete(&mut start, &mut evict_end);
        }
    }

    // --- B. Debounced scroll (see Section B) ---
    self.schedule_scroll();
}
```

**Key details:**
- `buffer.line_count()` returns `i32`. `LINE_CAP` is `i32` to match.
- `buffer.iter_at_line(EVICT_BATCH)` returns `Option<TextIter>`. The `if let Some(...)` guard handles the edge case where `line_count <= EVICT_BATCH` (should never happen given the `> LINE_CAP` check, but required by the API).
- After `buffer.delete()`, the iterators passed in are moved to the new merge position. No dangling iterator risk because they are local to the if-block.
- The eviction runs **before** the scroll scheduling so the scroll target is already correct when the scroll fires.

#### No changes to `clear()`

`clear()` calls `buffer.set_text("")` which resets to an empty buffer. Line count returns to 1. No eviction needed.

---

### B. Debounce `scroll_mark_onscreen`

**File:** `src/ui/log_panel.rs`

#### Struct changes

```rust
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

#[derive(Clone)]
pub struct LogPanel {
    pub expander: gtk::Expander,
    text_view: gtk::TextView,
    scroll_mark: gtk::TextMark,
    scroll_pending: Rc<Cell<bool>>,   // ← NEW: debounce flag
}
```

#### `new()` — add field initialisation

```rust
Self {
    expander,
    text_view,
    scroll_mark,
    scroll_pending: Rc::new(Cell::new(false)),   // ← NEW
}
```

#### New helper method `schedule_scroll()`

```rust
fn schedule_scroll(&self) {
    // If a scroll is already scheduled, do nothing.
    if self.scroll_pending.get() {
        return;
    }
    self.scroll_pending.set(true);

    let pending = self.scroll_pending.clone();
    let weak_view = self.text_view.downgrade();
    let mark_name = "scroll-end";

    glib::timeout_add_local_once(Duration::from_millis(80), move || {
        pending.set(false);
        if let Some(view) = weak_view.upgrade() {
            let buffer = view.buffer();
            if let Some(mark) = buffer.mark(mark_name) {
                // Keep the mark at the true end before scrolling.
                buffer.move_mark(&mark, &buffer.end_iter());
                view.scroll_mark_onscreen(&mark);
            }
        }
    });
}
```

**Key details:**
- `glib::timeout_add_local_once` fires once after ~80 ms. No `SourceId` to manage; the one-shot nature eliminates cancellation complexity.
- `self.text_view.downgrade()` returns `glib::WeakRef<gtk::TextView>`. The closure will be a no-op if the widget is destroyed before the timer fires.
- The `buffer.mark("scroll-end")` lookup inside the callback is safe because the buffer is owned by the `TextView` (retrieved from the upgraded `WeakRef`).
- 80 ms is below a typical 100 ms human perception threshold for scroll lag, while coalescing bursts of lines that arrive at e.g. 1000 lines/sec (would otherwise trigger 80 scroll calls per tick period).
- The `scroll_mark` field on the struct is retained for the `move_mark` call inside the callback.

#### Remove direct `scroll_mark_onscreen` call from `append_line`

The previous direct call:

```rust
buffer.move_mark(&self.scroll_mark, &buffer.end_iter());
self.text_view.scroll_mark_onscreen(&self.scroll_mark);
```

is replaced entirely by `self.schedule_scroll()`.

#### `clear()` — flush pending scroll

When clearing the log, any pending scroll should be harmlessly superseded. The `scroll_pending` flag is intentionally left as-is — if a timer fires after `clear()`, the mark lookup will still succeed and `scroll_mark_onscreen` on an empty buffer is a no-op. No special handling required.

---

### C. Drop Fake Progress Bar → Spinner Only

**File:** `src/ui/update_row.rs`

#### Fields to REMOVE

```rust
progress_bar: gtk::ProgressBar,                         // REMOVE
progress_timer: Rc<RefCell<Option<glib::SourceId>>>,    // REMOVE
progress_fraction: Rc<Cell<f64>>,                       // REMOVE
```

#### Fields to KEEP

```rust
spinner: gtk::Spinner,   // already present, no change
```

#### Imports to remove (if now unused)

After removing `progress_timer`, check whether `glib::SourceId` is still imported. If `progress_timer` was the only user of `glib::SourceId`, remove that import. The `use gtk::glib;` import at the top and `use std::time::Duration;` can be removed if `glib::timeout_add_local` is the only remaining user — verify after removing the timer.

After removal, `progress_fraction: Rc<Cell<f64>>` used `std::cell::Cell`. If no other field uses `Cell`, the import can be narrowed but is likely still needed (the existing `check_epoch` uses it in window.rs; within update_row.rs itself, check).

#### `new()` — remove ProgressBar construction and suffix

Remove:
```rust
// REMOVE these lines:
let progress_bar = gtk::ProgressBar::builder()
    .visible(false)
    .valign(gtk::Align::Center)
    .width_request(100)
    .build();
// ...
row.add_suffix(&progress_bar);
// ...
Self {
    // ...
    progress_bar,          // REMOVE
    progress_timer: Rc::new(RefCell::new(None)),  // REMOVE
    progress_fraction: Rc::new(Cell::new(0.0)),   // REMOVE
}
```

#### `set_status_checking()` — no ProgressBar references, keep as-is

Already fine:
```rust
pub fn set_status_checking(&self) {
    self.spinner.set_visible(true);
    self.spinner.set_spinning(true);
    self.status_label.set_label("Checking...");
    self.status_label.set_css_classes(&["dim-label"]);
}
```

(Remove the `self.progress_bar.set_visible(false)` line that exists there.)

#### `set_status_running()` — new implementation

```rust
pub fn set_status_running(&self) {
    self.spinner.set_visible(true);
    self.spinner.set_spinning(true);
    self.status_label.set_label("Updating...");
    self.status_label.set_css_classes(&["accent"]);
}
```

The entire timer setup block is removed.

#### `stop_progress_timer()` — REMOVE entirely

This method exists solely to stop the fake timer. It is gone with the progress bar.

#### All `set_status_*` methods — remove ProgressBar and timer lines

For each of `set_status_available`, `set_status_success`, `set_status_error`, `set_status_skipped`, `set_status_unknown`:

Remove any line that references:
- `self.progress_bar.*`
- `self.stop_progress_timer()`

The `self.spinner.set_visible(false); self.spinner.set_spinning(false);` lines are **kept** in all non-Running terminal states.

#### New `set_status_available()` — correct form

```rust
pub fn set_status_available(&self, count: usize) {
    self.spinner.set_visible(false);
    self.spinner.set_spinning(false);
    if count == 0 {
        self.status_label.set_label("Up to date");
        self.status_label.set_css_classes(&["success"]);
    } else {
        self.status_label.set_label(&format!("{count} available"));
        self.status_label.set_css_classes(&["accent"]);
    }
}
```

(No change needed here; it already has no ProgressBar refs.)

---

## 4. Files to Modify

| File | Changes |
|------|---------|
| `src/ui/log_panel.rs` | Add `LINE_CAP`, `EVICT_BATCH` constants; add `scroll_pending: Rc<Cell<bool>>` field; add `schedule_scroll()` method; update `append_line()` to evict and use debounced scroll; add `use std::cell::Cell; use std::rc::Rc; use std::time::Duration;` imports |
| `src/ui/update_row.rs` | Remove `progress_bar`, `progress_timer`, `progress_fraction` fields and all references; remove `stop_progress_timer()` method; simplify `set_status_running()`; remove `glib::timeout_add_local` usage |

No changes to:
- `src/ui/window.rs`
- `src/ui/mod.rs`
- `src/backends/mod.rs`
- `Cargo.toml` (no new dependencies)
- `meson.build`, `flake.nix`, `io.github.up.json`

---

## 5. Complete Code Sketches

### 5.1 `src/ui/log_panel.rs` — full revised file

```rust
use gtk::glib;
use gtk::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

/// Maximum number of lines retained in the log panel buffer.
const LINE_CAP: i32 = 5_000;
/// Number of lines to evict from the top when the cap is exceeded.
const EVICT_BATCH: i32 = 100;

#[derive(Clone)]
pub struct LogPanel {
    pub expander: gtk::Expander,
    text_view: gtk::TextView,
    scroll_mark: gtk::TextMark,
    scroll_pending: Rc<Cell<bool>>,
}

impl LogPanel {
    pub fn new() -> Self {
        let text_view = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(8)
            .right_margin(8)
            .css_classes(vec!["card"])
            .build();

        let scrolled = gtk::ScrolledWindow::builder()
            .min_content_height(200)
            .max_content_height(400)
            .child(&text_view)
            .build();

        let expander = gtk::Expander::builder()
            .label("Terminal Output")
            .margin_top(12)
            .child(&scrolled)
            .build();

        let buffer = text_view.buffer();
        let end_iter = buffer.end_iter();
        let scroll_mark = buffer.create_mark(Some("scroll-end"), &end_iter, false);

        Self {
            expander,
            text_view,
            scroll_mark,
            scroll_pending: Rc::new(Cell::new(false)),
        }
    }

    pub fn append_line(&self, line: &str) {
        let clean = strip_ansi(line);
        let buffer = self.text_view.buffer();
        let mut end = buffer.end_iter();
        buffer.insert(&mut end, &clean);
        buffer.insert(&mut end, "\n");

        // FIFO eviction: keep buffer at most LINE_CAP lines.
        if buffer.line_count() > LINE_CAP {
            let mut start = buffer.start_iter();
            if let Some(mut evict_end) = buffer.iter_at_line(EVICT_BATCH) {
                buffer.delete(&mut start, &mut evict_end);
            }
        }

        // Debounced scroll to bottom.
        self.schedule_scroll();
    }

    pub fn clear(&self) {
        let buffer = self.text_view.buffer();
        buffer.set_text("");
    }

    /// Schedules a single scroll-to-bottom, coalescing rapid calls into one.
    fn schedule_scroll(&self) {
        if self.scroll_pending.get() {
            return;
        }
        self.scroll_pending.set(true);

        let pending = self.scroll_pending.clone();
        let weak_view = self.text_view.downgrade();

        glib::timeout_add_local_once(Duration::from_millis(80), move || {
            pending.set(false);
            if let Some(view) = weak_view.upgrade() {
                let buffer = view.buffer();
                if let Some(mark) = buffer.mark("scroll-end") {
                    buffer.move_mark(&mark, &buffer.end_iter());
                    view.scroll_mark_onscreen(&mark);
                }
            }
        });
    }
}

/// Remove ANSI/VT100 escape sequences from `s`.
///
/// Handles:
/// - CSI sequences: ESC `[` followed by parameter bytes (`0x30–0x3F`),
///   intermediate bytes (`0x20–0x2F`), and a final byte (`0x40–0x7E`).
/// - Simple two-byte ESC sequences: ESC followed by any ASCII letter.
///
/// Any other byte sequence starting with ESC is passed through unchanged
/// rather than silently discarding legitimate content.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('[') => {
                chars.next();
                for ch in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&ch) {
                        break;
                    }
                }
            }
            Some(ch) if ch.is_ascii_alphabetic() => {
                chars.next();
            }
            _ => {
                out.push('\x1b');
            }
        }
    }
    out
}
```

### 5.2 `src/ui/update_row.rs` — revised struct and key methods

**Struct:**

```rust
use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

use crate::backends::Backend;

#[derive(Clone)]
pub struct UpdateRow {
    pub row: adw::ExpanderRow,
    status_label: gtk::Label,
    spinner: gtk::Spinner,
    pkg_rows: Rc<RefCell<Vec<adw::ActionRow>>>,
}
```

**`new()`:**

```rust
pub fn new(backend: &dyn Backend) -> Self {
    let status_label = gtk::Label::builder()
        .label("Ready")
        .css_classes(vec!["dim-label"])
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(30)
        .build();

    let spinner = gtk::Spinner::builder().visible(false).build();

    let icon = gtk::Image::from_icon_name(backend.icon_name());

    let row = adw::ExpanderRow::builder()
        .title(backend.display_name())
        .subtitle(backend.description())
        .build();

    row.add_prefix(&icon);
    row.add_suffix(&spinner);
    row.add_suffix(&status_label);

    Self {
        row,
        status_label,
        spinner,
        pkg_rows: Rc::new(RefCell::new(Vec::new())),
    }
}
```

**`set_status_running()`:**

```rust
pub fn set_status_running(&self) {
    self.spinner.set_visible(true);
    self.spinner.set_spinning(true);
    self.status_label.set_label("Updating...");
    self.status_label.set_css_classes(&["accent"]);
}
```

**`set_status_success()`:**

```rust
pub fn set_status_success(&self, count: usize) {
    self.spinner.set_visible(false);
    self.spinner.set_spinning(false);
    let msg = if count == 0 {
        "Up to date".to_string()
    } else {
        format!("{count} updated")
    };
    self.status_label.set_label(&msg);
    self.status_label.set_css_classes(&["success"]);
}
```

**`set_status_error()`:**

```rust
pub fn set_status_error(&self, msg: &str) {
    self.spinner.set_visible(false);
    self.spinner.set_spinning(false);
    self.status_label.set_label(&format!("Error: {}", msg));
    self.status_label.set_css_classes(&["error"]);
}
```

**`set_status_skipped()`:**

```rust
pub fn set_status_skipped(&self, msg: &str) {
    self.spinner.set_visible(false);
    self.spinner.set_spinning(false);
    self.status_label.set_label(msg);
    self.status_label.set_css_classes(&["dim-label"]);
}
```

**`set_status_unknown()`:**

```rust
pub fn set_status_unknown(&self, msg: &str) {
    self.spinner.set_visible(false);
    self.spinner.set_spinning(false);
    self.status_label.set_label(msg);
    self.status_label.set_css_classes(&["dim-label"]);
}
```

**`stop_progress_timer()` — DELETE this method entirely.**

---

## 6. Import Audit

### `src/ui/log_panel.rs`

Add to top of file:

```rust
use gtk::glib;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;
```

`gtk::prelude::*` already provides `TextBufferExt` (for `line_count`, `iter_at_line`, `delete`, `insert`, `move_mark`, `mark`) and `TextViewExt` (for `scroll_mark_onscreen`, `buffer`). No new GTK imports needed.

### `src/ui/update_row.rs`

Remove or trim the following (if no longer used):

- `use std::cell::Cell;` — `Cell` was used only by `progress_fraction: Rc<Cell<f64>>`. After removal, `Cell` is unused. **Remove from import.**
- `use std::time::Duration;` — `Duration` was used only by `glib::timeout_add_local(Duration::from_millis(200), ...)`. After removing the timer, `Duration` is unused. **Remove from import.**
- `glib::SourceId` — was used as the type of `progress_timer`'s inner value. After removal, `glib::SourceId` is unused. Verify whether `glib` is still needed; the `use gtk::glib;` import is still needed for `glib::ControlFlow` if present elsewhere, but since we're removing the `timeout_add_local` call, verify whether `glib` is referenced at all. If not, remove the `use gtk::glib;` import too.

After removal, the needed imports are:

```rust
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use crate::backends::Backend;
```

(`gtk` types like `gtk::Label`, `gtk::Spinner`, `gtk::pango`, `gtk::Image`, `gtk::Align` are accessed via `adw::prelude::*` which re-exports GTK prelude, or via the `gtk::` path directly. Verify the actual import requirements by checking what compiles.)

---

## 7. Risks and Mitigations

### Risk 1: `iter_at_line` returns `None` unexpectedly

**Scenario:** `buffer.iter_at_line(EVICT_BATCH)` returns `None` if `EVICT_BATCH >= buffer.line_count()`.  
**Mitigation:** The check `if buffer.line_count() > LINE_CAP` ensures at least `LINE_CAP + 1` lines exist before attempting eviction. Since `LINE_CAP = 5000` and `EVICT_BATCH = 100`, `buffer.line_count()` is always ≥ 5001 when eviction runs. `iter_at_line(100)` will never return `None` in practice. The `if let Some(...)` pattern is defensive and correct per the API contract.

### Risk 2: `buffer.delete` invalidates the `scroll_mark` iterator

**Scenario:** The named mark `"scroll-end"` anchors to a position in the buffer. When text at the top is deleted, the mark's position shifts.  
**Mitigation:** GTK `TextMark` positions are automatically updated when text is deleted — the mark moves with the surrounding text. Since `"scroll-end"` is placed at the end of the buffer (gravity `false` = right gravity), it will remain at the end after deletion. The `schedule_scroll()` callback also calls `buffer.move_mark(&mark, &buffer.end_iter())` before scrolling, so any drift is corrected.

### Risk 3: Borrow checker conflict in `schedule_scroll` closure

**Scenario:** The closure captures `weak_view` (a `glib::WeakRef<gtk::TextView>`), which is `'static`. The `pending` is an `Rc<Cell<bool>>`, also `'static`. Both are moved into the `FnOnce` closure. Since `glib::timeout_add_local_once` requires `F: 'static`, these captures must not borrow from `self`.  
**Mitigation:** Both captures are owned values (clones of `Rc` / `WeakRef`). No borrow of `self` survives past the end of `schedule_scroll()`. This satisfies `'static` and the borrow checker.

### Risk 4: `scroll_pending` flag not reset on `clear()`

**Scenario:** `clear()` is called while a scroll timer is pending. The timer fires, finds an empty buffer, calls `scroll_mark_onscreen` on an empty view — harmless. Then `pending.set(false)`, allowing the next `append_line()` after clear to schedule a new scroll correctly.  
**Mitigation:** No explicit reset of `scroll_pending` in `clear()` is needed. The in-flight timer will self-clear the flag.

### Risk 5: `LogPanel` clone and `scroll_pending` shared state

**Scenario:** `LogPanel` derives `Clone`. Multiple clones share the same `Rc<Cell<bool>>` for `scroll_pending`. If two clones call `append_line()` simultaneously (not possible — GTK is single-threaded), there is no race. If separate clones are used in different parts of the UI pointing to the same logical log, they share the pending flag correctly.  
**Mitigation:** This is the desired behaviour. All clones of a single `LogPanel` instance coalesce their scroll requests into one. This is consistent with how the existing `text_view`, `scroll_mark`, and `expander` are shared across clones via GTK's GObject reference counting.

### Risk 6: `update_row.rs` — timer not cancelled on widget drop

**Scenario (current bug):** In the current code, if an `UpdateRow` is dropped while `set_status_running` is active but before any terminal state (`set_status_success`, `set_status_error`, etc.) is called, the repeating `glib::timeout_add_local` timer continues to fire indefinitely (accessing a dropped `progress_bar` clone — which is a GObject with reference counting, so the clone keeps it alive, but the row itself is gone). This is a subtle resource leak.  
**Mitigation (after fix):** Since the entire timer mechanism is removed, this bug is eliminated by design. No timer to leak.

### Risk 7: `adw::Spinner` unavailability

**Scenario:** Developer or reviewer assumes `adw::Spinner` should be used.  
**Mitigation:** Confirmed: `Cargo.toml` specifies `features = ["v1_5"]`. `adw::Spinner` requires `v1_6`. It is NOT available. The implementation must use `gtk::Spinner` exclusively.

---

## 8. Implementation Steps (for Implementation Subagent)

1. **Open** `src/ui/log_panel.rs`.
2. Add `use gtk::glib;`, `use std::cell::Cell;`, `use std::rc::Rc;`, `use std::time::Duration;` imports.
3. Add `LINE_CAP` and `EVICT_BATCH` constants.
4. Add `scroll_pending: Rc<Cell<bool>>` to the `LogPanel` struct.
5. Initialise `scroll_pending` in `LogPanel::new()`.
6. Replace the two lines at the end of `append_line()` (mark move + `scroll_mark_onscreen`) with the FIFO eviction block + `self.schedule_scroll()` call.
7. Add the `schedule_scroll()` private method.

8. **Open** `src/ui/update_row.rs`.
9. Remove `progress_bar: gtk::ProgressBar` field declaration.
10. Remove `progress_timer: Rc<RefCell<Option<glib::SourceId>>>` field declaration.
11. Remove `progress_fraction: Rc<Cell<f64>>` field declaration.
12. Remove `ProgressBar::builder()...build()` construction block from `new()`.
13. Remove `row.add_suffix(&progress_bar)` from `new()`.
14. Remove `progress_bar`, `progress_timer`, `progress_fraction` from `Self { ... }` constructor.
15. Delete the entire `stop_progress_timer()` method.
16. Rewrite `set_status_running()` to contain only spinner show + label.
17. Remove all `self.progress_bar.*` and `self.stop_progress_timer()` calls from `set_status_checking()`, `set_status_available()`, `set_status_success()`, `set_status_error()`, `set_status_skipped()`, `set_status_unknown()`.
18. Remove now-unused imports: `use std::cell::Cell;`, `use std::time::Duration;`. Verify if `use gtk::glib;` is still needed; remove if not.

9. **Run** `cargo build` — must succeed with zero errors.
10. **Run** `cargo clippy -- -D warnings` — must produce zero warnings.
11. **Run** `cargo fmt --check` — must produce no diffs.
