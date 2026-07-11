# Package List Popover — Specification

## Current State Analysis

The "updated packages" list for each backend (NixOS, Flatpak, Homebrew, fwupd, etc.)
is implemented in [`src/ui/update_row.rs`](../../../src/ui/update_row.rs):

- `UpdateRow` (`update_row.rs:7-23`) wraps an `adw::ExpanderRow` (`update_row.rs:9,54-57`).
- `set_packages()` (`update_row.rs:134-164`) populates the expander with one
  `adw::ActionRow` per package name via `row.add_row()`, capped at
  `MAX_PACKAGES = 50` (`update_row.rs:148`) with a synthetic "… and N more" row
  appended when truncated (`update_row.rs:156-163`).
- Each `UpdateRow` is added to the `backends_group` `adw::PreferencesGroup`
  ("Sources") in `src/ui/window.rs` (`window.rs:263-267, 876, 1063`).
- That group lives inside the single page-wide `gtk::ScrolledWindow`
  (`window.rs:173-176`, `vexpand(true)`, `hscrollbar_policy: Never`) that wraps
  the whole update page (hero, system info, sources, log panel).

**Problem:** When an `ExpanderRow` expands with many child rows, it grows the
page's content box in place. Since there is no separate scroll region for the
package list, the *whole page's* `ScrolledWindow` has to scroll to reveal the
expanded rows — this reads as cluttered/scrollbar-heavy and was flagged by the
user as undesirable.

## Problem Definition

Replace the inline `ExpanderRow` package list with a `gtk::Popover` that
floats above the page, anchored to a small button on each backend's row, so
that opening the package list never changes the page's height and never
triggers the outer `ScrolledWindow`.

## Proposed Solution Architecture

- Replace `adw::ExpanderRow` in `UpdateRow` with a plain `adw::ActionRow`
  (title/subtitle unchanged) plus a `gtk::MenuButton` added as a row suffix.
  `gtk::MenuButton` is the idiomatic gtk4-rs pairing for a button that opens a
  `gtk::Popover` — it owns the anchoring, click-to-toggle, and outside-click
  dismissal automatically (confirmed against gtk4-rs docs via Context7; no
  manual `popup()`/`popdown()`/`set_parent()` bookkeping needed).
- The `MenuButton`'s label reflects the package count (e.g. "42 pkgs"), matches
  the `pill-btn` mockup styling via a CSS class, and is hidden/disabled when
  there are zero packages (mirrors current `set_enable_expansion(!packages.is_empty())`
  behavior).
- The popover's content is a `gtk::Box` (vertical) containing:
  - A small heading label ("NixOS — 42 packages").
  - A `gtk::ListBox` populated with one row per package name (replaces
    `adw::ActionRow` children — plain `gtk::Label`-based rows are lighter
    weight and sufficient since these rows are display-only).
  - The `ListBox` is wrapped in a `gtk::ScrolledWindow` with a fixed
    `max-content-height` (e.g. 320px) so a popover never grows taller than
    that regardless of package count — this is the *only* remaining scroll
    surface, and it is scoped to the popover, never to the main page.
- Keep the existing `MAX_PACKAGES = 50` display cap and "… and N more" row —
  behavior is unchanged, only the container changes.
- No new crate dependency: `gtk::Popover` and `gtk::MenuButton` are already
  part of the `gtk4` crate already in use (`Cargo.toml`); this is a new
  *widget type* for this codebase but not a new *dependency*.

## Implementation Steps

1. In `src/ui/update_row.rs`:
   - Change `pub row: adw::ExpanderRow` to `pub row: adw::ActionRow`.
   - Remove `pkg_rows: Rc<RefCell<Vec<adw::ActionRow>>>` tracking of
     `ExpanderRow` children; replace with tracking needed to rebuild the
     popover's `ListBox` content on each `set_packages()` call (e.g. keep a
     handle to the `ListBox` and call a clear-then-repopulate pattern, same
     shape as today).
   - Add a `gtk::MenuButton` (stored on the struct) as a row suffix, with a
     `gtk::Popover` assigned via `menu_button.set_popover(Some(&popover))`.
   - Update `set_packages()` to: set the `MenuButton`'s label to `"{count} pkgs"`,
     set `menu_button.set_sensitive(!packages.is_empty())` (instead of
     `set_enable_expansion`), and repopulate the popover's internal `ListBox`.
   - Remove `self.row.set_expanded(false)` (no longer applicable); no
     replacement needed since a popover simply won't open when insensitive.
2. In `src/ui/window.rs`: no changes expected — `backends_group.add(&row.row)`
   works identically since `row.row` is still a single widget (now
   `adw::ActionRow` instead of `adw::ExpanderRow`), added the same way.
3. Add a small CSS class (e.g. `.pkg-count-pill`) if needed to match the
   pill-button visual style, following the existing `up-hero`/`vex-sources-group`
   pattern already in the stylesheet (`data/` CSS resource).

## Dependencies

- No new crates. `gtk::Popover`, `gtk::MenuButton`, `gtk::ListBox`,
  `gtk::ScrolledWindow` all already ship in the `gtk4` crate pinned in
  `Cargo.toml`.
- Verified via Context7 (`/gtk-rs/gtk4-rs`): `gtk::MenuButton::set_popover()`
  is the documented, idiomatic way to pair a button with a `gtk::Popover` in
  gtk4-rs; no deprecated APIs involved.

## Configuration Changes

None required (no D-Bus policy, GResource manifest, or desktop-file changes —
this is UI-only and does not add new gettext strings beyond the existing
count-label pattern already covered by `po/POTFILES.in`).

## Risks and Mitigations

- **Risk:** Very large package batches (80-100+, e.g. big NixOS rebuilds)
  still need internal scrolling inside the popover.
  **Mitigation:** Acceptable per user-approved design (Option 1 in the
  presented mockups) — this is a rare edge case, and the existing 50-item
  cap already limits how many rows are ever rendered.
- **Risk:** `gtk::Popover` is a new widget type in this codebase; behavior
  (autohide, keyboard nav, screen-reader semantics) should be checked against
  the existing `adw::AlertDialog` dialogs' accessibility conventions.
  **Mitigation:** Use `gtk::MenuButton` (not manual `Popover::popup()`) so
  standard GTK accessibility/focus behavior is inherited for free.
- **Risk:** Removing `ExpanderRow` changes the row's implicit "click to expand"
  affordance users may be used to.
  **Mitigation:** Matches the user-approved mockup exactly; the `MenuButton`
  pill communicates interactivity clearly (button + count + chevron glyph).
