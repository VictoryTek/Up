# WIRE_DISK_ESTIMATE — Review

Spec: `.github/docs/subagent_docs/WIRE_DISK_ESTIMATE_spec.md`
Scope: MASTER_PLAN item 19.

## Modified files

- `src/ui/update_row.rs` — `last_estimated_size` cell + `set_estimated_size` /
  `last_estimated_size`; reset in `set_status_checking`.
- `src/ui/window.rs` — availability check now also calls
  `estimate_size(&executor)`; `CheckPayload` is a 3-tuple; per-row size stored;
  status label gains a ` (~<size>)` suffix; new `low_space_banner` +
  `maybe_warn_low_space` helper (off-thread `df`).
- `src/backends/mod.rs` — `#[allow(dead_code)]` removed from `estimate_size`.
- `src/disk.rs` — `#[allow(dead_code)]` removed from `detect_available_space`,
  `parse_df_available`, `format_bytes` (all now live).

## Findings

- **Feature delivered** — "N updates available (~450 MB)" and a low-disk
  banner, both driven by real per-backend estimates.
- Aggregate uses `.reduce(saturating_add)` so the suffix appears only when a
  backend actually estimated something; pacman/nix/homebrew (no override,
  default `None`) contribute nothing and cause no spurious "~0 B".
- Banner is advisory only — never gates the update; hidden at the start of
  every check cycle so a freed-up disk clears it on recheck.
- `df` probe runs via `spawn_background_async`; no new GTK-thread blocking.
- `disk.rs` is now fully de-suppressed (advances item 15 — only
  `changelog.rs` remains).
- No new unit tests: the disk parsers already have coverage; the additions are
  UI glue not unit-testable without GTK.

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
| Functionality | 94% | A |
| Code Quality | 95% | A |
| Security | 100% | A |
| Performance | 96% | A |
| Consistency | 97% | A |
| Build Success | 100% | A |

**Overall Grade: A (97%)**

## Result

PASS
