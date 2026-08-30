# DEADCODE_SUPPRESSION — Review

Spec: `.github/docs/subagent_docs/DEADCODE_SUPPRESSION_spec.md`
Scope: MASTER_PLAN item 15 — `src/disk.rs` only.

## Modified files

- `src/disk.rs` — removed `#![allow(dead_code)]`; added `#[allow(dead_code)]`
  (with an item-19 comment) to `detect_available_space`, `parse_df_available`,
  `format_bytes`.

## Findings

- Removing the blanket suppression flagged exactly the three functions the
  spec predicted — the low-disk-space-warning helpers, unused until item 19.
  Everything else in `disk.rs` (`parse_size_value`, `parse_dnf_size_line`,
  `parse_apt_size`, `parse_dnf_size`, `parse_zypper_size`,
  `parse_flatpak_sizes`, `parse_fwupd_size`) is live via backend
  `estimate_size` and needs no annotation.
- No behaviour change; no signatures touched.
- `changelog.rs` / `snapshot.rs` intentionally left module-suppressed
  (fully dead, pending items 20 / 18) — recorded in the spec and MASTER_PLAN.
- `check.rs`, `config.rs`, `history.rs`, `ui/history_page.rs` were already
  de-suppressed by items 2/6/7.

## Build validation (`nix develop`)

| Command | Result |
|---|---|
| `cargo fmt --check` | pass |
| `cargo clippy -- -D warnings` | pass (0 warnings) |
| `cargo build` | pass |
| `cargo test` | 149 passed / 0 failed |
| `scripts/preflight.sh` | exit 0 |

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

PASS
