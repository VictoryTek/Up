# Package List Popover — Review

## Scope

Reviewed against [`package_list_popover_spec.md`](./package_list_popover_spec.md).
Modified files: `src/ui/update_row.rs`, `data/style.css`.

## Specification Compliance

- `adw::ExpanderRow` → `adw::ActionRow` + `gtk::MenuButton` + `gtk::Popover`: done exactly as specced.
- `gtk::MenuButton::set_popover()` used (idiomatic gtk4-rs pairing, verified via Context7) — no manual `popup()`/`popdown()` bookkeeping.
- Popover content: heading label + `gtk::ListBox` in a `gtk::ScrolledWindow` capped at `max_content_height(320)` — matches spec.
- `MAX_PACKAGES = 50` cap and "… and N more" summary row preserved unchanged.
- `backends_group.add(&row.row)` in `window.rs` required no changes, as predicted (single-widget contract preserved).
- Orphaned `pkg_rows: Rc<RefCell<Vec<adw::ActionRow>>>` field and now-unused `RefCell` import removed (surgical cleanup of self-caused orphans only, per project convention).
- Orphaned `row.expander` CSS rule removed since `adw::ExpanderRow` is no longer used anywhere in the codebase (confirmed via `grep -rn "ExpanderRow" src/`); replaced with a `.vex-sources-group row:hover` rule scoped to the same group so the hover highlight isn't broadened to unrelated rows elsewhere in the app, plus a new `.pkg-count-pill` class for the button.

## Best Practices / Consistency

- Widget construction follows existing builder-pattern style used throughout `update_row.rs`.
- No new crate dependency — `gtk::Popover`/`gtk::MenuButton`/`gtk::ListBox` all ship in the already-pinned `gtk4` crate.
- CSS scoping follows the existing `.vex-sources-group` / `.up-hero` convention rather than unscoped element selectors.

## Completeness

- Empty-package case handled: `menu_button.set_visible(false)` mirrors the prior `set_enable_expansion(false)` behavior (button disappears instead of showing a disabled row).
- Popover heading dynamically reflects backend name + count on every `set_packages()` call, so repeated checks don't show stale headings.

## Security / Performance

- No new I/O, no new external input parsing — pure UI widget swap. No security-relevant surface changed.
- `ListBox` clear-and-repopulate on each check is O(n) in package count, same complexity as the previous `ExpanderRow` approach.

## Build Validation

Run via `nix develop --command bash scripts/preflight.sh` (repo is on NixOS; GTK4 libs only available inside the flake dev shell, per project constraints):

```
--- Step 1: Formatting check (cargo fmt --check) ---     PASS
--- Step 2: Lint check (cargo clippy -- -D warnings) ---  PASS
--- Step 3: Build verification (cargo build) ---          PASS
--- Step 3b: Build daemon crate (cargo build -p up-daemon) --- PASS
--- Step 4: Test execution (cargo test) ---               PASS (106 passed; 0 failed)
--- Step 5: desktop-file-validate ---                     skipped (tool not installed)
--- Step 6: appstreamcli validate ---                     skipped (tool not installed)
--- Step 7: cargo audit ---                                skipped (tool not installed)
--- Step 8: nix flake check ---                            PASS
All preflight checks passed.
```

One `cargo fmt --check` diff was found on first run (multi-line `format!` wrapping in `set_packages`) and fixed via `cargo fmt`; re-run confirmed clean.

Note: `cargo clippy --all-targets -- -D warnings` (test binaries included) surfaces a pre-existing `await_holding_lock` lint in `src/backends/nix.rs:1077,1115` — confirmed via `git diff --stat` and `git log -- src/backends/nix.rs` to predate this change and be outside its scope (not touched by this diff, last modified in unrelated prior commits `c6495c3`/`85cb83c`/`f9f022e`). The project's actual preflight/CI lint step (`cargo clippy -- -D warnings`, no `--all-targets`) does not compile test binaries and passes cleanly.

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 100% | A |
| Best Practices | 100% | A |
| Functionality | 100% | A |
| Code Quality | 100% | A |
| Security | 100% | A |
| Performance | 100% | A |
| Consistency | 100% | A |
| Build Success | 100% | A |

**Overall Grade: A (100%)**

## Result: PASS

No refinement cycle needed. Proceeding to Phase 6 (already run above as part of build validation) — preflight passed, so this is delivery-ready.
