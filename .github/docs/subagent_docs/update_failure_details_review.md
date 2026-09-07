# Update Failure Details — Review

## Scope Reviewed

- `src/ui/log_panel.rs` — `backend_tail()` + pure `tail_for_backend()` helper + tests
- `src/ui/update_row.rs` — `set_status_error(msg, log_tail)`
- `src/ui/window.rs` — two `set_status_error` call sites

## Findings

- **Spec compliance:** matches the confirmed scope. Details button now always
  shown on failure; dialog text = error message + (deduplicated) recent backend
  output. No auto-open, no panel resize — as requested.
- **Simplicity:** parsing logic extracted to a pure free function so it is unit
  tested without a GTK harness, consistent with the repo's test-only-pure-logic
  convention (no existing `src/ui` tests touch GTK).
- **Consistency:** prefix `[<kind>] ` reuses the exact `format!` string every
  `BackendLog` handler already emits; `Display for BackendKind` is the single
  source of truth.
- **Dedup:** `msg.contains(tail)` prevents doubling output for
  `BackendError::Exit`, whose message already embeds the tail.
- **i18n:** one new string `"Recent output:"`; both touched files are already in
  `po/POTFILES.in`. No `po/meson.build` change needed.
- **Performance:** one bounded buffer scan per failure; buffer is line-capped at
  5 000. Negligible.
- **Security:** no new inputs, no new process execution, read-only buffer access.

## Build Validation (`scripts/preflight.sh`, via `nix develop`)

```
Step 1 cargo fmt --check ............ pass
Step 2 cargo clippy -D warnings ..... pass
Step 3 cargo build (+ up-daemon) .... pass
Step 4 cargo test .................. 162 passed; 0 failed
Step 8 nix flake check ............. pass
Steps 5-7 (desktop-file-validate / appstreamcli / cargo-audit) — tools absent, skipped (CI covers)
All preflight checks passed.
```

New tests: `ui::log_panel::tests::{returns_matching_lines_without_prefix,
truncates_to_last_max_lines, empty_when_no_match}` — all pass.

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

## Result

**PASS** — no refinement required. Preflight passed.
