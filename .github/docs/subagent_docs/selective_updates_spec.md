# Spec: Make per-package selective updates real (item 9)

## Current state analysis

- `src/backends/mod.rs:196-223`: `Backend` trait already declares
  `supports_item_selection() -> bool` (default `false`) and
  `run_selected_update(items: &[String], runner) -> UpdateResult`
  (default delegates to `run_update`). Doc comment on
  `supports_item_selection` (line 199-202) is explicit about the intended
  UI contract: *"When `false`, per-item checkboxes in the UI are rendered
  read-only (always checked, non-interactive)."* — i.e. checkboxes should
  always exist, just be non-interactive when unsupported, not omitted.
  `run_selected_update`'s doc comment states its caller contract:
  *"Callers guarantee: `items.is_empty()` is never true when this method
  is called."*
- Overridden (`supports_item_selection() -> true`) by: `AptBackend`,
  `DnfBackend`, `ZypperBackend` (`os_package_manager.rs`, each with its
  own item-validation — e.g. Apt's `run_selected_update` at line 140
  rejects any item containing characters outside
  `[A-Za-z0-9-+._:]` or over 255 chars), `FlatpakBackend`
  (`flatpak.rs:220`), `HomebrewBackend` (`homebrew.rs:88`), and the Nix
  backend (`nix.rs:754`, presumably flake-attr-validated — not modified
  here). `PacmanBackend` and `PluginBackend` do **not** override it, so
  they fall back to the trait default (`false` / full `run_update`) —
  correct existing behavior, unaffected by this change.
- `src/orchestrator.rs:136-161` (`UpdateOrchestrator::run_all`): already
  iterates `(backend, selected_items)` pairs and dispatches:
  `Some(items) if backend.supports_item_selection() && !items.is_empty()
  => run_selected_update(items, ...)`, else `run_update(...)`. Fully
  built, zero changes needed here — it already enforces "never call
  `run_selected_update` with an empty slice" by falling through to
  `run_update` if `items` is empty, which is exactly why the UI side must
  never *intend* an empty selection to mean "run everything" (see Risks).
- `src/ui/window.rs` (previously line ~409, now ~473 after items 1-8's
  edits): the "Update All" backend list is built as
  `detected_borrow.iter().filter(|b| !skipped).cloned().map(|b| (b,
  None)).collect()` — always `None`, so `run_selected_update` is never
  reachable from the primary Update All flow regardless of what a
  backend supports.
- `src/ui/update_row.rs`: `set_packages(&self, packages: &[String])`
  (lines 185-222) populates `self.popover_list` with plain
  `gtk::Label` rows (package name only, no checkbox), capped at
  `MAX_PACKAGES = 50` with a `"… and N more"` summary label for the
  remainder — there is currently no way to select/deselect anything.
  `UpdateRow` (struct, line 7-30) has no field tracking per-package
  selection state, and `UpdateRow::new(backend: &dyn Backend, ...)`
  already receives the `Backend` trait object it could call
  `supports_item_selection()` on, but doesn't retain that.
- The retry-loop closure (`window.rs`, the per-row Retry button) builds
  `UpdateOrchestrator::new(vec![(backend, None)])` for a single backend —
  **not** cited in the master plan's file list for this item (only
  `mod.rs:196-223`, `window.rs:409`, `update_row.rs:124-154` are listed).
  Left as `None` (full retry), matching current behavior and keeping this
  change scoped to the cited files — a retry is already a
  narrower, deliberate re-run of one backend; carrying forward a stale
  partial selection into a retry is a separate design question not asked
  for here.

## Problem definition

Full backend + orchestrator plumbing for selective per-package updates
exists end-to-end, but the UI never lets the user pick a subset — the
Update All handler always passes `None`, so `run_selected_update` is dead
code from the UI's perspective.

## Proposed solution

1. `UpdateRow` gains a checkbox per displayed package (up to the existing
   50-item display cap), defaulting to checked. Checkbox interactivity is
   gated on **both** conditions needing to hold: the backend supports
   selection, and the full package list fits within the display cap (if
   it doesn't, some packages have no checkbox at all and could never be
   deselected — showing interactive checkboxes for only the *visible*
   subset would silently misrepresent what "selected" means once
   `run_selected_update` is actually invoked with a partial, cap-truncated
   list). When either condition fails, checkboxes are still shown
   (matching the trait's own documented UI contract) but forced
   insensitive and always checked — visually communicating "you can't
   filter this one" rather than hiding the control.
2. `UpdateRow::selected_items() -> Option<Vec<String>>` — returns `None`
   whenever selection doesn't meaningfully apply (unsupported, capped, no
   list loaded yet, or every checkbox is checked — "everything selected"
   is behaviorally identical to a full update, so there's no reason to
   route through `run_selected_update` for it). Otherwise returns exactly
   the checked item IDs — **never an empty `Vec`** (see below).
3. The Update All backend-list construction (window.rs) is changed from
   an unconditional `filter` + `map(|b| (b, None))` to a `filter_map`
   that, for each non-skipped backend: looks up its row, calls
   `selected_items()`, and — critically — if the result is `Some(items)`
   with `items.is_empty()` (user unchecked every visible package without
   using the row-level skip checkbox), treats that exactly like a skip:
   marks the row `set_status_skipped("No packages selected")` and
   **excludes the backend from this run's backend list entirely**, rather
   than passing `Some(vec![])` through to the orchestrator (which would
   silently fall back to a *full* `run_update` per the orchestrator's own
   `!items.is_empty()` guard — the opposite of what an all-unchecked
   selection means to the user). This is the one behavior the spec must
   get right per `run_selected_update`'s documented caller contract.

## Implementation steps

1. `src/ui/update_row.rs`:
   - Add `use std::cell::RefCell;` alongside the existing `use
     std::cell::Cell;`.
   - Add three fields to `UpdateRow`: `supports_selection: bool`,
     `package_checks: Rc<RefCell<Vec<(String, gtk::CheckButton)>>>`,
     `selection_capped: Rc<Cell<bool>>`.
   - In `UpdateRow::new`, set `supports_selection =
     backend.supports_item_selection()`; initialize the other two fields
     empty/`false`.
   - Rewrite `set_packages`: clear `package_checks` alongside the
     existing popover-list clear; for each of the up to 50 displayed
     packages, build a small `gtk::Box` containing a `gtk::CheckButton`
     (`.active(true)`, `.sensitive(interactive)` where `interactive =
     supports_selection && packages.len() <= MAX_PACKAGES`) plus the
     existing package-name `gtk::Label`, append it to `popover_list`
     instead of the bare label, and push `(pkg.clone(), checkbox)` into
     `package_checks`. Set `selection_capped` to
     `packages.len() > MAX_PACKAGES`. The existing `"… and N more"`
     summary row for the truncated remainder is unchanged.
   - Add `pub fn selected_items(&self) -> Option<Vec<String>>`: `None` if
     `!supports_selection || selection_capped.get()`; `None` if
     `package_checks` is empty (no list loaded); otherwise collect
     checked item IDs and return `None` if that equals the full count,
     else `Some(checked_ids)` (may legitimately be an empty `Vec` here —
     the caller in `window.rs` is responsible for treating that as "skip
     this backend for this run", per the orchestrator's caller contract).
2. `src/ui/window.rs`: replace the `filter(...).cloned().map(|b| (b,
   None))` chain (building the Update All backend list) with a
   `filter_map` that looks up each backend's row, skips it as before if
   `row.is_skipped()`, calls `row.selected_items()`, and:
   - `Some(items) if items.is_empty()` → call
     `row.set_status_skipped("No packages selected")`, exclude from the
     run.
   - otherwise → include as `(backend, selected)` (where `selected` may
     be `None` or `Some(non_empty_items)`).

## Dependencies

None — no new crates.

## Configuration changes

None.

## Risks and mitigations

- **Risk:** Passing `Some(vec![])` through to the orchestrator would
  silently run a *full* update for a backend the user explicitly
  deselected everything from (orchestrator's own `!items.is_empty()`
  guard falls through to `run_update`). **Mitigation:** exactly why
  `window.rs`'s backend-list construction special-cases an empty
  `Some(items)` result as "exclude from this run" rather than forwarding
  it — this is the central correctness requirement of this spec.
- **Risk:** Showing interactive checkboxes for only the visible 50 items
  when a backend has more than that would let a user believe they
  filtered out everything except a few packages, while the hidden
  remainder still updates (or doesn't) unpredictably. **Mitigation:**
  checkboxes are forced non-interactive (and implicitly "all selected")
  whenever `packages.len() > MAX_PACKAGES`, so `selected_items()` always
  returns `None` (full update) in that case — no partial-selection claim
  is ever made when the full list isn't representable in the UI.
- **Risk:** A backend not overriding `supports_item_selection` (Pacman,
  plugin backends) getting non-functional-looking checkboxes.
  **Mitigation:** exactly the trait's own documented behavior — checkbox
  shown, always checked, insensitive; `selected_items()` returns `None`
  unconditionally for these since `supports_selection` is `false`, so
  `run_update()` (full update) is always used, matching current behavior
  exactly.
- **Risk:** Per-backend item-ID validation (e.g. Apt's shell-token regex
  in `run_selected_update`) rejecting a package name the UI itself put in
  the list. **Mitigation:** out of scope — item IDs come from
  `list_available()`'s own output for that backend, already
  backend-specific and unrelated to this UI-wiring change; any mismatch
  there is a pre-existing backend-parsing concern, not something this
  spec touches.
- **Risk:** Retry-loop and `spawn_cache_bypass` still pass `None`
  unconditionally. **Mitigation:** intentional — not in the master plan's
  cited scope for this item; retry already means "try this one backend
  fully again," and cache-bypass is a distinct VexOS-specific flow.
