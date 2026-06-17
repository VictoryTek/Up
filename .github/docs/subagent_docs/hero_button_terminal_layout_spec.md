# Spec: Hero Button Layout & Terminal Height Fix

## Current State Analysis

### Hero Area (`src/ui/window.rs` ~191–222)
```
[icon 52px] [title "System Updater" + subtitle]   (horizontal, spacing 14)
```
Update All and Cancel buttons live in a separate `footer_box` beneath the scrolled content area,
centered horizontally (lines 626–635). This wastes vertical space with an extra row for the buttons.

### Log Panel (`src/ui/log_panel.rs` ~34–38)
The inner `ScrolledWindow` has `min_content_height(150)` and `vexpand(true)`.

In `window.rs` (lines 643–648) the expander is wired so that `set_vexpand(true)` is applied when
the expander is opened. This causes it to compete with the main scrolled content area for space,
stealing height from the content above it and pushing things up instead of extending the window.

## Problem Definition

1. Expanded terminal consumes all available vertical space, covering scrollable content above it.
2. Separate footer row for Update All / Cancel wastes vertical space.
3. User wants the Update All and Cancel buttons inline with the hero (icon + title row), right-aligned,
   saving one full row of vertical space.

## Proposed Solution

### 1. Move buttons into the hero row (right-aligned)

Restructure `hero_box` to use three logical sections:
```
[icon] [title+subtitle]  [<hexpand spacer>]  [Cancel] [Update All]
```

- Remove `halign: Center` and `margin_top: 12` from `update_button` (footer positioning).
- Add a `valign: Center` to both buttons so they align with the icon/text vertically.
- Add an hexpand `gtk::Box` (or `gtk::Label`) as a spacer between the text and buttons.
- Wrap [Cancel] + [Update All] in a small horizontal box (spacing 8), right-aligned.
- Remove `footer_box` entirely and its `page_box.append` call.

### 2. Cap terminal height, stop stealing from content above

- Add `max_content_height(200)` to the `ScrolledWindow` inside `LogPanel::new()`.
- Remove the `connect_notify_local("expanded", ...)` handler from `window.rs` that toggled
  `set_vexpand` on the expander. Without it, the expander grows to at most 200 px and stops.
- Keep `vexpand(true)` on the scrolled window so it fills up to max within that cap.

This means expanded terminal = fixed 150–200 px strip at the bottom, no content displacement.

## Implementation Steps

1. `src/ui/log_panel.rs` — add `.max_content_height(200)` to `ScrolledWindow` builder.
2. `src/ui/window.rs`:
   a. Remove `.halign(gtk::Align::Center)` and `.margin_top(12)` from `update_button` builder.
   b. Add `.valign(gtk::Align::Center)` to both `update_button` and `cancel_button`.
   c. Create `hero_button_box`: horizontal Box, spacing 8, valign Center.
   d. Append `cancel_button` then `update_button` to `hero_button_box`.
   e. Create `hero_spacer`: hexpand=true Label or Box.
   f. Append `hero_spacer` and `hero_button_box` to `hero_box` (after existing icon+text children).
   g. Delete `footer_box` construction and `page_box.append(&footer_box)`.
   h. Delete the `expander.connect_notify_local("expanded", ...)` block.

## Dependencies

No new crates. Pure GTK4 layout changes. Context7 not required (no new external libraries).

## Risks and Mitigations

- **Risk:** Buttons too cramped in the hero row on small windows.
  **Mitigation:** `pill` buttons are compact; 760px default width is ample. Hero already has
  `padding: 24px 12px 8px` from CSS — buttons fit naturally.
- **Risk:** Terminal height change breaks log readability.
  **Mitigation:** 150–200 px gives ~8–12 lines of monospace output, sufficient for live progress.
  The save button still allows exporting full log.
