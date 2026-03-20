# Spec: Finding #8 — LogPanel Per-Line TextMark Allocation

**Feature Name:** `log_panel_textmark`
**Spec File:** `.github/docs/subagent_docs/log_panel_textmark_spec.md`
**Affected File:** `src/ui/log_panel.rs`

---

## 1. Current State Analysis

### Affected Code

File: [src/ui/log_panel.rs](../../../../src/ui/log_panel.rs)

```rust
pub fn append_line(&self, line: &str) {
    let buffer = self.text_view.buffer();
    let mut end = buffer.end_iter();
    buffer.insert(&mut end, line);
    buffer.insert(&mut end, "\n");

    // Auto-scroll to bottom  [LINES 44-47 — THE ISSUE]
    let mark = buffer.create_mark(None, &buffer.end_iter(), false);  // line 44
    self.text_view.scroll_mark_onscreen(&mark);                        // line 45
    buffer.delete_mark(&mark);                                          // line 46
}
```

### Current `LogPanel` Struct

```rust
#[derive(Clone)]
pub struct LogPanel {
    pub expander: gtk::Expander,
    text_view: gtk::TextView,
}
```

### Call Sites

Both call sites call `clear()` before entering the log streaming loop, then call `append_line()` for each received line:

- `src/ui/window.rs` — update page: `log_clone.clear()` then `log_ref2.append_line(&format!("[{kind}] {line}"))` in `glib::spawn_future_local`
- `src/ui/upgrade_page.rs` — upgrade page: `log_clone.clear()` then `log_ref.append_line(...)` in `glib::spawn_future_local`

---

## 2. Problem Definition

### What Actually Happens

On every `append_line()` call:
1. `buffer.create_mark(None, &buffer.end_iter(), false)` allocates a new `GObject` (a `GtkTextMark`) on the GTK object heap.
2. `scroll_mark_onscreen()` uses it.
3. `buffer.delete_mark(&mark)` explicitly removes the mark from the buffer and drops the reference, triggering deallocation.

The mark **is** explicitly deleted on each call, so marks do **not** permanently accumulate. The finding's description that marks are "never explicitly deleted" is inaccurate — the deletion is present.

### The Real Performance Issue

Each `append_line()` call causes:
- **1 GObject allocation** (`gtk_text_mark_new` + buffer insertion into its internal mark tree)
- **1 GTK signal emission** (`mark-set` signal on `create_mark`)
- **1 GTK signal emission** (`mark-deleted` signal on `delete_mark`)
- **1 GObject deallocation** (GObject reference count drop to zero)

This is unnecessary overhead on every line of terminal output. A typical system update may produce 50–500+ lines. An OS-level upgrade may produce thousands.

### Root Cause

The mark used for scrolling is a transient implementation detail. It is created only to immediately call `scroll_mark_onscreen()`, then discarded. A single named mark stored as a struct field is sufficient for this purpose, reused across all calls via `buffer.move_mark()`.

---

## 3. Proposed Solution Architecture

### Approach: Store One Persistent Named Mark in `LogPanel`

Add a `scroll_mark: gtk::TextMark` field to `LogPanel`. Initialize it once in `new()`. In `append_line()`, move it to the new end position and scroll to it. The mark is owned by the `TextBuffer` but the struct holds a reference to it.

**Why this is safe:**
- `gtk::TextMark` is a reference-counted GObject wrapper (implements `Clone`). `#[derive(Clone)]` on `LogPanel` continues to work — cloning just increments GTK's reference count.
- `TextMark` does not need to be `Send` or `Sync`; `LogPanel` is only ever accessed from the GTK main thread.
- The mark survives `buffer.set_text("")` (called by `clear()`). GTK does not delete marks when text is deleted — marks are repositioned to offset 0 and remain valid in the buffer. See §6 for verification.

### Mark Gravity

`create_mark(Some("scroll-end"), &end_iter, false)`

- `left_gravity: false` = **right gravity**: the mark stays to the right of text inserted at its current position.
- This is the correct gravity for an end-of-buffer scroll mark. After `buffer.insert()` appends text at the end iterator position, a right-gravity mark placed there beforehand would be advanced past the inserted text. However, since we explicitly call `buffer.move_mark()` to the new `end_iter` after insertion, gravity is not functionally critical — the explicit move is the mechanism.

---

## 4. Verified API Signatures

Source: [gtk-rs.org TextBufferExt](https://gtk-rs.org/gtk4-rs/stable/latest/docs/gtk4/prelude/trait.TextBufferExt.html)

```rust
// TextBufferExt — on gtk::TextBuffer
fn create_mark(
    &self,
    mark_name: Option<&str>,
    where_: &TextIter,
    left_gravity: bool,
) -> TextMark

fn move_mark(&self, mark: &impl IsA<TextMark>, where_: &TextIter)

fn delete_mark(&self, mark: &impl IsA<TextMark>)

fn mark(&self, name: &str) -> Option<TextMark>
```

Source: [gtk-rs.org TextViewExt](https://gtk-rs.org/gtk4-rs/stable/latest/docs/gtk4/prelude/trait.TextViewExt.html)

```rust
// TextViewExt — on gtk::TextView (current usage, unchanged)
fn scroll_mark_onscreen(&self, mark: &impl IsA<TextMark>)
// "Scrolls self the minimum distance such that mark is contained
//  within the visible area of the widget."

// Alternative with finer control (not used here):
fn scroll_to_mark(
    &self,
    mark: &impl IsA<TextMark>,
    within_margin: f64,
    use_align: bool,
    xalign: f64,
    yalign: f64,
)
```

`scroll_mark_onscreen` is the correct choice — it scrolls the minimum distance to make the mark visible, which is the desired behaviour for a terminal log auto-scroll.

---

## 5. Implementation Steps

### Step 1 — Add `scroll_mark` field to `LogPanel`

```rust
#[derive(Clone)]
pub struct LogPanel {
    pub expander: gtk::Expander,
    text_view: gtk::TextView,
    scroll_mark: gtk::TextMark,  // ADD THIS
}
```

### Step 2 — Initialize the mark once in `new()`

After `text_view` is built and before `Self { ... }`:

```rust
let buffer = text_view.buffer();
let end_iter = buffer.end_iter();
let scroll_mark = buffer.create_mark(Some("scroll-end"), &end_iter, false);
```

Return it in the struct literal:

```rust
Self { expander, text_view, scroll_mark }
```

### Step 3 — Replace per-call `create_mark`/`delete_mark` in `append_line()`

**Before:**
```rust
pub fn append_line(&self, line: &str) {
    let buffer = self.text_view.buffer();
    let mut end = buffer.end_iter();
    buffer.insert(&mut end, line);
    buffer.insert(&mut end, "\n");

    let mark = buffer.create_mark(None, &buffer.end_iter(), false);
    self.text_view.scroll_mark_onscreen(&mark);
    buffer.delete_mark(&mark);
}
```

**After:**
```rust
pub fn append_line(&self, line: &str) {
    let buffer = self.text_view.buffer();
    let mut end = buffer.end_iter();
    buffer.insert(&mut end, line);
    buffer.insert(&mut end, "\n");

    buffer.move_mark(&self.scroll_mark, &buffer.end_iter());
    self.text_view.scroll_mark_onscreen(&self.scroll_mark);
}
```

### Step 4 — No changes required to `clear()`

```rust
pub fn clear(&self) {
    let buffer = self.text_view.buffer();
    buffer.set_text("");
    // scroll_mark is NOT deleted by set_text("") — see §6
}
```

---

## 6. `clear()` and Mark Lifecycle — Critical Analysis

The original conversational research incorrectly noted that `buffer.set_text("")` would invalidate the stored mark. The corrected analysis:

### What `set_text("")` Does

Per the `TextBufferExt::set_text` documentation:
> "Deletes current contents of @self, and inserts @text instead."

`set_text` calls GTK's internal `delete` operation on the entire buffer range, followed by inserting `""`. This is a **text deletion**, not a call to `TextBufferExt::delete_mark`.

### How GTK Handles Marks During Text Deletion

When text is deleted from a `TextBuffer`:
- Marks **within** the deleted range are moved to the **start position** of the deleted range
- Marks are **not** removed from the buffer
- `TextMark::is_deleted()` returns `false` — the mark is still valid

After `buffer.set_text("")`:
- All text is deleted. Our `scroll_mark` is repositioned to offset 0.
- The buffer is then effectively empty (contains only `""`).
- The mark is still registered with the buffer under the name `"scroll-end"`.
- The `gtk::TextMark` struct field `self.scroll_mark` still refers to the same valid GObject.

When `append_line()` is called next, `buffer.move_mark(&self.scroll_mark, &buffer.end_iter())` moves it to the new end position. No re-creation needed.

**Explicit mark removal** only occurs via:
- `buffer.delete_mark(&mark)` — explicit call
- Buffer being dropped entirely

Neither happens in the `clear()` → `append_line()` usage pattern.

---

## 7. Dependencies and Configuration Changes

**No new dependencies.** This fix uses only APIs already present in the `gtk4` crate version in use (`gtk4 = "0.9"` in `Cargo.toml`):
- `TextBufferExt::create_mark` — already called in current code
- `TextBufferExt::move_mark` — same trait, no new import
- `TextViewExt::scroll_mark_onscreen` — already called in current code

No changes to `Cargo.toml`, `meson.build`, `flake.nix`, or any data files.

---

## 8. Risks and Mitigations

| Risk | Severity | Mitigation |
|------|----------|------------|
| `set_text("")` invalidating the stored mark | **NOT A RISK** — text deletion does not remove marks from buffer | Verified against GTK `TextBuffer` del semantics |
| Mark name collision (`"scroll-end"`) if LogPanel is instantiated twice | Low | Two independent `LogPanel` instances create two independent `TextBuffer` objects; mark names are scoped to their buffer, not globally |
| Thread safety | Not applicable | `LogPanel` and all GTK operations occur exclusively on the GTK main thread; `glib::spawn_future_local` ensures this |
| `#[derive(Clone)]` broken by new field | Not a risk | `gtk::TextMark` implements `Clone` (GObject ref-count clone) |
| Scroll behaviour difference | None | `buffer.move_mark()` + `scroll_mark_onscreen()` produces identical scrolling behaviour to the current `create_mark()` + `scroll_mark_onscreen()` + `delete_mark()` pattern |

---

## 9. Research Sources

1. **`src/ui/log_panel.rs`** — Current source code, full 55-line file; bug identified at lines 44–46
2. **`src/ui/window.rs`** — Usage of `LogPanel` in update page; `clear()` + `append_line()` calling pattern confirmed
3. **`src/ui/upgrade_page.rs`** — Usage in upgrade page; same pattern
4. **`src/ui/mod.rs`** — Module declarations; `spawn_background_async` utility
5. **[gtk-rs.org: `TextBufferExt` trait](https://gtk-rs.org/gtk4-rs/stable/latest/docs/gtk4/prelude/trait.TextBufferExt.html)** — Exact Rust signatures for `create_mark`, `move_mark`, `delete_mark`, `mark`, `set_text`; mark gravity semantics; `delete_mark` docs confirming explicit removal requirement
6. **[gtk-rs.org: `TextViewExt` trait](https://gtk-rs.org/gtk4-rs/stable/latest/docs/gtk4/prelude/trait.TextViewExt.html)** — Exact Rust signatures for `scroll_mark_onscreen` and `scroll_to_mark`; confirmed `scroll_mark_onscreen` minimally scrolls to make mark visible
7. **[GTK4 TextBuffer struct page (gtk-rs.org)](https://gtk-rs.org/gtk4-rs/stable/latest/docs/gtk4/struct.TextBuffer.html)** — Confirmed `mark-set` and `mark-deleted` signals (emitted on every `create_mark`/`delete_mark`); confirmed `TextBufferExt` and `TextBufferExtManual` trait names

---

## 10. Summary

The fix replaces 3 operations per `append_line()` call (GObject alloc + 2 signal emissions + GObject dealloc) with 1 operation (`move_mark`). The change is confined to `src/ui/log_panel.rs` only. It adds one field to `LogPanel`, modifies `new()` by 2 lines, and `append_line()` replaces 3 lines with 2. `clear()` is unchanged. No dependencies, no configuration, no build system changes.
