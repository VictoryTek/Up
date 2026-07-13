# Sources Row Suffix Reorder — Spec

## Current state analysis

`src/ui/update_row.rs`, `UpdateRow::new()` (adw::ActionRow suffix widgets, lines 111-116):

```rust
row.add_prefix(&icon);
row.add_suffix(&skip_checkbox);
row.add_suffix(&retry_button);
row.add_suffix(&spinner);
row.add_suffix(&status_label);
row.add_suffix(&menu_button);
```

`adw::ActionRow` lays out suffix widgets left-to-right in the order `add_suffix()` is
called. Current visual order (after the leading icon/title/subtitle):
skip checkbox → retry button → spinner → status label → popover (package list) button.

## Problem definition

User wants the "Sources" preferences group rows reversed so the popover package-list
button comes first, then the status/update message, then the checkbox — i.e. the
suffix order should be flipped end-to-end.

## Proposed solution

Reverse the five `add_suffix()` calls in place. Confirmed with user: the retry button
(only visible on check error) should stay paired with the checkbox as a control,
landing just before it. New order:

```rust
row.add_prefix(&icon);
row.add_suffix(&menu_button);
row.add_suffix(&status_label);
row.add_suffix(&spinner);
row.add_suffix(&retry_button);
row.add_suffix(&skip_checkbox);
```

Resulting visual order: popover button → status label → spinner → retry button → checkbox.

## Implementation steps

1. In `src/ui/update_row.rs`, reorder the five `row.add_suffix(...)` calls as above.
2. No other files, struct fields, signal handlers, or CSS need to change — only the
   call order changes, widget construction and logic are untouched.

## Dependencies

None (no new crates; internal reorder only, Context7 not applicable).

## Configuration changes

None.

## Risks and mitigations

- Risk: GTK/libadwaita may apply different margins/spacing based on suffix position
  (e.g. first/last suffix CSS classes). Mitigation: visually low risk since these are
  the same standard suffix widgets; build + preflight will catch any layout/lint
  regressions, and this is a purely cosmetic reorder with no logic change.
