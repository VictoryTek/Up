# Review: Make per-package selective updates real (item 9)

Spec: `.github/docs/subagent_docs/selective_updates_spec.md`

Modified files:
- `src/ui/update_row.rs`
- `src/ui/window.rs`

## Findings

1. **Specification Compliance** — Matches the spec exactly: checkboxes
   added to the popover package list (up to the 50-item display cap),
   forced non-interactive/all-checked whenever the backend doesn't
   support selection or the list exceeds the cap, `selected_items()`
   returns `None` for "nothing to filter" cases and the exact checked
   subset otherwise (including a legitimately empty `Vec`), and the
   Update All backend-list construction in `window.rs` special-cases an
   empty selection as "exclude from this run" rather than forwarding
   `Some(vec![])` to the orchestrator (which would silently run a full
   update — the central correctness requirement identified in Phase 1).
2. **Best Practices** — No new abstractions beyond what's needed; reuses
   the existing `Backend::supports_item_selection()` /
   `run_selected_update()` contract and the orchestrator's existing
   dispatch logic without modification.
3. **Consistency** — Checkbox-always-shown-but-insensitive-when-
   unsupported behavior matches the `Backend` trait's own doc comment on
   `supports_item_selection` verbatim ("per-item checkboxes in the UI are
   rendered read-only (always checked, non-interactive)").
4. **Maintainability** — `selected_items()` is a small, well-documented
   pure query method; the `window.rs` change is a single `filter_map`
   replacing a `filter` + `map`, same overall shape as before.
5. **Completeness** — All three cited files touched
   (`backends/mod.rs` needed no changes — already correct — confirmed in
   Phase 1); retry-loop and `spawn_cache_bypass` deliberately left
   passing `None`, matching the master plan's cited scope (only
   `window.rs`'s Update All backend-list construction, previously always
   `None`, was in scope).
6. **Performance** — No regression; `selected_items()` does a single
   linear scan over at most 50 checkboxes, only called once per backend
   per Update All click.
7. **Security** — No new attack surface; item IDs collected from
   checkboxes are exactly the strings `list_available()` already
   produced for that backend, and each backend's own
   `run_selected_update` already validates them (e.g. Apt's shell-token
   character allowlist, confirmed unchanged in Phase 1) before use.
8. **API Currency** — N/A.
9. **Build Validation:**

   `cargo build`:
   ```
   Compiling up v2.2.0 (/home/nimda/Projects/Up)
   Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.75s
   ```

   `cargo clippy -- -D warnings`: clean, no warnings.

   `cargo fmt --check`: clean, no diff.

   `cargo test`:
   ```
   test result: ok. 109 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
   ```
   No new automated test added for `selected_items()` — constructing a
   `gtk::CheckButton`/`gtk::Box` for a unit test requires an initialized
   GTK context (`gtk::init()`/display connection), which this codebase's
   test suite doesn't set up anywhere; `update_row.rs`, `window.rs`,
   `history_page.rs` all have zero `#[test]` blocks for the same reason,
   an existing, consistent boundary (also noted in the item 6/7/8
   reviews) between GTK-widget UI code and the pure backend/storage logic
   that *is* unit-tested.

   **Manual functional check:** launched `./target/debug/up` — ran 5
   seconds with no crash or error output before being stopped. Live
   interactive verification (opening a package popover, unchecking an
   item, confirming the Update All run excludes it) was not performed —
   same screenshot/input-injection tooling limitations noted in the item
   6-8 reviews apply here.

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 100% | A |
| Best Practices | 100% | A |
| Functionality | 90% | A- (build/lint/test green and clean process launch confirmed; live checkbox interaction not verified in this environment) |
| Code Quality | 100% | A |
| Security | 100% | A |
| Performance | 100% | A |
| Consistency | 100% | A |
| Build Success | 100% | A |

**Overall Grade: A (99%)**

## Result: PASS
