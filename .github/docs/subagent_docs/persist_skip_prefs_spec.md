# Spec: Persist skip-backend preferences across restarts (item 7)

## Current state analysis

- `src/config.rs` (`#![allow(dead_code)]`): fully working config layer.
  `AppConfig { skipped_backends: Vec<BackendKind>, snapshot_preference:
  SnapshotPreference }`, `load_config()` (returns `AppConfig::default()`
  on any error/missing file), `save_config()`. JSON file at
  `$XDG_CONFIG_HOME/up/config.json`. Zero callers anywhere (confirmed via
  grep). `BackendKind` already derives `Serialize, Deserialize, Clone,
  PartialEq, Eq` (`src/backends/mod.rs:72`), so it round-trips through
  JSON and supports `Vec::contains`/`.clone()` without changes.
- `src/ui/update_row.rs`: `UpdateRow::new(backend, on_skip_changed,
  on_retry)` always starts with `skip_flag = Rc::new(Cell::new(false))`
  (line 51) and a `gtk::CheckButton` built with no initial `.active(...)`
  (default false, lines 55-58) — there is currently no way to construct a
  row that starts pre-skipped. `connect_toggled` (line 122) is wired
  *after* the checkbox is built, so any state set before that connection
  point does not fire the toggled signal/`on_skip_changed`.
  `is_skipped()` (line 166) reads `skip_flag`.
- `src/ui/window.rs`: backend-detection completion handler (~line
  848-1086, the `glib::spawn_future_local` reacting to
  `detect_rx.recv()`) holds `let mut rows_mut = rows.borrow_mut();` for
  the entire per-backend population loop (~line 871-1074) while calling
  `UpdateRow::new(...)` for each detected backend, then
  `rows_mut.push((backend.kind(), row))` (line 1073). Because `rows_mut`
  holds a mutable borrow of the same `RefCell` that `on_skip_changed`
  later immutably borrows (`rows_cb.borrow()`, line 890), **triggering
  the toggled signal synchronously during this loop would panic** (double
  borrow on the same `RefCell<Vec<...>>`). This is why the fix must set
  the row's initial skip state *before* `connect_toggled` is wired inside
  `UpdateRow::new`, not by calling `set_active()` on an already-connected
  checkbox from the population loop.
  Existing consulters of `is_skipped()` already work correctly regardless
  of *how* the flag got set: `window.rs:441` (marks pre-skipped rows
  before "Update All" starts), and the `non_skipped_available`/
  `non_skipped_total` filters at lines 455, 808, 816, 893, 1056. No
  changes needed there.
  `on_skip_changed` is defined per-row at line 886-897 (the *first*
  closure argument to `UpdateRow::new`) and already borrows `rows_cb` to
  recompute `non_skipped_available` for button sensitivity — this is
  where a config save is naturally added, since it already has the full
  row list needed to compute the current skip set.
- `crate::config` is already `mod config;` in `main.rs:6`; no module-tree
  change needed.

## Problem definition

Skip-backend checkboxes are session-only — every restart resets all
backends to unskipped, even though the config load/save machinery
already exists and is fully implemented, just uncalled.

## Proposed solution

1. Load `AppConfig` once when backend detection completes (small,
   synchronous local JSON read on the GTK thread — consistent with other
   small synchronous calls already done in this handler; not worth
   plumbing through the background-thread detection channel for a few
   bytes).
2. Give `UpdateRow::new` a new `initial_skipped: bool` parameter. Use it
   to seed `skip_flag`'s initial value and the checkbox's `.active(...)`
   builder property *before* `connect_toggled` is wired, and to seed the
   status label to "Skipped" (mirroring the toggle handler's own skipped
   branch) so a restored row looks correct on first paint, not just
   functionally correct.
3. At the row-creation call site, pass `config.skipped_backends.contains(&backend.kind())`.
4. In the existing `on_skip_changed` closure (window.rs ~886-897), after
   computing `non_skipped_available` from `borrowed` (the existing
   `rows_cb.borrow()`), reload the config, overwrite `skipped_backends`
   with the current skip set from `borrowed`, and save. Reloading rather
   than threading a shared `Rc<RefCell<AppConfig>>` through every row
   closure keeps `snapshot_preference` (untouched by this change) intact
   without introducing new shared mutable state for a checkbox that's
   toggled rarely.

## Implementation steps

1. `src/ui/update_row.rs`:
   - Add `initial_skipped: bool` as the second parameter of
     `UpdateRow::new` (after `backend`, before `on_skip_changed`).
   - `let skip_flag = Rc::new(Cell::new(initial_skipped));`
   - Add `.active(initial_skipped)` to the `skip_checkbox` builder.
   - After `status_label` is built, if `initial_skipped`, set it to
     `"Skipped"` / `["dim-label"]` (same as the toggle handler's skipped
     branch) so pre-skipped rows render correctly immediately.
2. `src/ui/window.rs`:
   - In the backend-detection-completion async block, right after
     `backends_group.remove(&placeholder_row);`, add
     `let config = crate::config::load_config();`.
   - At the `UpdateRow::new(...)` call site (~line 884), pass
     `config.skipped_backends.contains(&backend.kind())` as the new
     second argument.
   - In the `on_skip_changed` closure body, after the existing
     `button_cb.set_sensitive(non_skipped_available > 0);` line, add:
     ```rust
     let mut cfg = crate::config::load_config();
     cfg.skipped_backends = borrowed
         .iter()
         .filter(|(_, r)| r.is_skipped())
         .map(|(k, _)| k.clone())
         .collect();
     let _ = crate::config::save_config(&cfg);
     ```
     (reuses the existing `borrowed` binding already in scope for the
     sensitivity calculation).
3. Remove `#![allow(dead_code)]` from `src/config.rs` now that it has
   callers.

## Dependencies

None — `serde`/`serde_json` already used by `config.rs`.

## Configuration changes

None to the schema; `config.json`'s `skipped_backends` field is already
declared and simply starts being written/read.

## Risks and mitigations

- **Risk:** Triggering the checkbox's `toggled` signal during row
  construction would panic (`RefCell` already mutably borrowed by the
  population loop's `rows_mut`). **Mitigation:** initial state is set via
  the `skip_flag` `Cell` and the checkbox builder's `.active(...)`
  property *before* `connect_toggled` is called — GTK's builder-time
  property assignment does not go through the signal machinery, and no
  handler is connected yet at that point, so no signal fires.
- **Risk:** `config.skipped_backends` referencing a `BackendKind::Plugin`
  ID for a plugin that's no longer installed/detected would silently do
  nothing (never matched against a detected backend). **Mitigation:**
  correct, intended behavior — `contains()` only affects currently
  detected backends; stale entries are harmless and get naturally
  dropped from `skipped_backends` the next time any checkbox is toggled
  (since the save always writes the *current* full skip set for
  currently-detected backends).
- **Risk:** Reloading config from disk on every toggle instead of caching
  is an extra file read per user click. **Mitigation:** negligible —
  this is a manually-clicked checkbox, not a hot path; avoids adding new
  shared mutable state across every row's closures for a rare event.
- **Risk:** `SnapshotPreference`/other future `AppConfig` fields being
  clobbered by a stale in-memory copy. **Mitigation:** exactly why we
  reload from disk immediately before mutating `skipped_backends` rather
  than keeping a stale cached `AppConfig` around from startup.
