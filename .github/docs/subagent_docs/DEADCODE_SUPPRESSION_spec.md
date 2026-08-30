# DEADCODE_SUPPRESSION — Specification

MASTER_PLAN item 15 — remove blanket `#![allow(dead_code)]` from the
abandoned-subsystem modules, incrementally, as each is wired up or deleted.

## Current state (verified 2026-08-29)

| Module | `#![allow(dead_code)]`? | Wired up by |
|---|---|---|
| `src/check.rs` | already removed | item 2 |
| `src/config.rs` | already removed | item 7 |
| `src/history.rs` | already removed | item 6 |
| `src/ui/history_page.rs` | already removed | item 6 |
| `src/disk.rs` | **present** | item 11 (partial — `parse_*_size` now used by backend `estimate_size`) |
| `src/changelog.rs` | present | item 20 (not started) |
| `src/snapshot.rs` | present | item 18 (not started) |

Four of the seven modules were already de-suppressed when their subsystems
were wired up. `disk.rs` became partially live via item 11: every
`parse_apt_size` / `parse_dnf_size` / `parse_zypper_size` /
`parse_flatpak_sizes` / `parse_fwupd_size` (and their helpers `parse_size_value`,
`parse_dnf_size_line`) is now called from a backend `estimate_size`
implementation.

## Scope of this pass

`src/disk.rs` only. `changelog.rs` and `snapshot.rs` stay module-suppressed —
they are 100% dead (no external callers; private `mod` with zero references)
and are not being wired up or deleted here; de-suppressing them would mean
tagging ~12 / ~5 individual symbols with no benefit until items 20 / 18.

## Change

- Delete the file-level `#![allow(dead_code)]` from `src/disk.rs`.
- Add targeted `#[allow(dead_code)]` to the three symbols that are still
  genuinely unused — `detect_available_space`, `parse_df_available`,
  `format_bytes` (the low-disk-space-warning trio, pending item 19) — each
  with a comment pointing at item 19.

Result: `cargo clippy -- -D warnings` now covers all live code in `disk.rs`;
any *new* unused item in that module will be flagged instead of hidden.

## Risks & mitigations

Negligible — no behaviour change, no signatures touched. `cargo clippy -D
warnings` + full `cargo test` gate it.

## Success criteria

- `disk.rs` has no module-level `#![allow(dead_code)]`.
- Exactly the three item-19 functions carry `#[allow(dead_code)]`.
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo build`,
  `cargo test` clean; `scripts/preflight.sh` exits 0.
- MASTER_PLAN item 15 marked partial (disk done; changelog/snapshot pending
  items 20/18).
