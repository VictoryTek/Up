# WIRE_CHANGELOG — Review

Spec: `.github/docs/subagent_docs/WIRE_CHANGELOG_spec.md`
Scope: MASTER_PLAN item 20.

## Modified files

- `src/changelog.rs` — `#![allow(dead_code)]` removed; new
  `supports_changelog(&BackendKind)`.
- `src/ui/update_row.rs` — `UpdateRow` holds `kind`, `changelog_button`,
  `packages`; button shown/hidden across the status transitions and the skip
  toggle; new `show_changelog_dialog()`.

## Findings

- **Feature delivered** — per-row "What's new" button for the seven backends
  with changelog support; async fetch off the GTK thread; scrollable
  monospace dialog with clean empty / error fallbacks.
- Button visibility is consistent: only when `count > 0` and
  `supports_changelog(kind)`; hidden while checking/running/unknown and when
  the row is skipped.
- Package list stored uncapped (the popover's 50-item cap doesn't limit the
  changelog query).
- `run_cmd` keeps its direct `tokio::process::Command` — deliberate, it needs
  the `tokio::time::timeout` wrapper `probe` lacks; documented in the spec.
- New UI strings left un-gettext'd to match surrounding code (item 25).
- **Completes MASTER_PLAN item 15** — `changelog.rs` was the last
  `#![allow(dead_code)]` module.
- No new unit tests — the fetchers spawn real package-manager processes and
  the wiring is GTK glue; `supports_changelog` is a trivial `matches!`.

## Build validation (`nix develop`)

| Command | Result |
|---|---|
| `cargo fmt --check` | pass |
| `cargo clippy -- -D warnings` | pass (0 warnings) |
| `cargo build` | pass |
| `cargo test` | 151 passed / 0 failed |
| `scripts/preflight.sh` | exit 0 |

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 100% | A |
| Best Practices | 95% | A |
| Functionality | 93% | A |
| Code Quality | 95% | A |
| Security | 100% | A |
| Performance | 96% | A |
| Consistency | 96% | A |
| Build Success | 100% | A |

**Overall Grade: A (97%)**

## Result

PASS
