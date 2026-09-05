# Terminal Output Panel — Fill Wasted Bottom Space

## Current State Analysis

- `src/ui/window.rs::build_update_page()` builds the Update page as:
  `page_box` → `scrolled` (page-level `GtkScrolledWindow`, `vexpand(true)`) →
  `clamp` (`adw::Clamp`) → `content_box` (vertical `GtkBox`) →
  `[hero_box, progress_bar, sys_info_group, backends_group, log_panel.expander]`.
- None of `content_box`'s children set `vexpand`, so `content_box` only
  requests its natural (summed) height. GTK's default `GtkViewport` (auto-
  inserted by the page-level `GtkScrolledWindow`) then gives `content_box`
  only that natural height, even when the window itself is much taller
  (the window is sized to ~90% of the monitor height in
  `default_window_size()`). The leftover vertical space in the viewport is
  left blank below the log panel — this is the empty area visible in the
  screenshot.
- `src/ui/log_panel.rs::LogPanel::new()` builds the "Terminal Output" panel
  as `expander` → `toast_overlay` → `scrolled` (inner `GtkScrolledWindow`,
  `min_content_height(72)`, `max_content_height(180)`) → `text_view`.
  Commit `ba90019` shrank this box's `min_content_height` from 150 to 72 to
  stop the Update page from overflowing into a scroll bar on a 1280×800
  display. That fix is still needed and must not be reverted.
- The regression reported by the user: on any screen taller than the
  1280×800 worst case, the terminal box now sits at its small floor height
  and the rest of the extra window height renders as dead space beneath it,
  instead of being given to the terminal panel.

## Problem Definition

The Terminal Output panel does not grow to use the vertical space the
window actually has available; it only grows in response to appended log
content up to a fixed 180px cap. On larger screens this leaves a large,
empty, unusable gap at the bottom of the Update page.

## Proposed Solution

Let the Terminal Output panel absorb the window's leftover vertical space
using standard GTK4 expand/allocate semantics, instead of relying solely on
fixed pixel bounds:

- Set `vexpand(true)` on the inner terminal `GtkScrolledWindow` in
  `src/ui/log_panel.rs`.
- GTK4 propagates a descendant's `vexpand` up through ordinary containers
  (`GtkBox`, `AdwToastOverlay`, `GtkExpander`, `AdwClamp`, `GtkViewport`)
  automatically via `compute_expand`, as none of the intermediate widgets in
  this tree pin their own expand flag. This causes the page-level
  `GtkScrolledWindow`'s viewport to give `content_box` its full allocated
  height (instead of just its natural/summed height), and `content_box`
  (a `GtkBox`) gives that extra height to the one child that asked for it —
  `log_panel.expander`.
- `min_content_height(72)` is left unchanged and continues to act as the
  floor, preserving the 1280×800 no-scroll-bar fix from `ba90019`. On a
  short window, `content_box`'s natural height already consumes the
  viewport's allocation, so there is no leftover space for the expand child
  to claim and behavior is unchanged from today.
- `max_content_height(180)` is left in place; it only bounds the scrolled
  window's *natural size request* (relevant when `propagate-natural-height`
  is enabled, which it is not here), so it does not fight the `vexpand`
  fill behavior.

This is a one-property change confined to `src/ui/log_panel.rs`, requires no
new dependency, and does not touch `window.rs` sizing logic or the
short-screen `.compact` breakpoint.

## Implementation Steps

1. In `src/ui/log_panel.rs`, add `.vexpand(true)` to the terminal
   `gtk::ScrolledWindow::builder()` call.
2. Rebuild and visually confirm (build only; no live display in this
   environment) that no other widget in the chain sets an explicit
   `vexpand(false)`/`hexpand`-set override that would block propagation
   (confirmed by reading `window.rs` and `log_panel.rs` — none do).

## Dependencies

None. Uses only existing `gtk4`/`libadwaita` widget properties already in
use elsewhere in the codebase (`gtk::ScrolledWindow::builder().vexpand(...)`
pattern already used at `window.rs:252-255` for the page-level scrolled
window). No Context7 lookup required per the Dependency Policy exemption
for internal changes with no new dependencies.

## Configuration Changes

None.

## Risks and Mitigations

- **Risk:** Terminal panel could grow unbounded on very tall/4K monitors.
  **Mitigation:** This matches the explicit user request (fill the wasted
  space); acceptable since the panel is itself independently scrollable
  once its content exceeds the allocated height.
- **Risk:** Reintroducing the 1280×800 scroll-bar regression that `ba90019`
  fixed. **Mitigation:** `vexpand` only grants *extra* space beyond the
  natural/minimum layout; on a short window there is no extra space to
  grant, so the floor (`min_content_height(72)`) and overall page height
  budget are unchanged from the current, already-verified-fixed state.
- **Risk:** `AdwExpander`/`AdwToastOverlay` might not propagate `vexpand`
  as assumed. **Mitigation:** Verified via `cargo build` after the change;
  if the panel visually fails to grow this would only be confirmable with a
  live display, which is called out explicitly in the review as an
  untested-UI-verification limitation.
