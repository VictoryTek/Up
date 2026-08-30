# WIRE_DISK_ESTIMATE — Specification

MASTER_PLAN item 19 — wire up disk-size estimation in the update rows.
Source: ARCH M10, BUGS M3, FEATURES 6.

## Problem

`Backend::estimate_size()` is implemented for APT/DNF/Zypper/Flatpak/fwupd/
plugins (routed through `CommandExecutor` since item 11) but nothing calls it.
`src/disk.rs::{detect_available_space, parse_df_available, format_bytes}` are
likewise unused. The UI shows "12 updates available" with no size context and
no low-disk-space warning before an unattended update run.

## Design

### Per-backend estimate collected during the availability check

`src/ui/window.rs` availability-check cycle (`run_checks`):

- The per-backend background task already calls `count_available` +
  `list_available`; add `estimate_size(&executor)` → `Option<u64>`. The
  `CheckPayload` tuple gains a third element.
- On the GTK side, store it on the row: `UpdateRow::set_estimated_size(Option<u64>)`.

### `UpdateRow`

- New `last_estimated_size: Rc<Cell<Option<u64>>>` field with
  `set_estimated_size` / `last_estimated_size` accessors; reset to `None` in
  `set_status_checking()`.

### Aggregate display

When the last check completes (`remaining == 0`):

- Sum `last_estimated_size()` over non-skipped rows
  (`Option<u64>` via `.reduce(saturating_add)` — `None` when no backend
  estimated anything).
- If there are updates and the sum is `Some(n > 0)`, append
  ` (~<formatted>)` to the "N updates available" status label, using
  `disk::format_bytes`.

### Low-disk-space banner

- New `low_space_banner: adw::Banner` (next to `metered_banner`), hidden by
  default, hidden again at the start of every check cycle.
- `maybe_warn_low_space(&banner, needed: Option<u64>)`: runs
  `disk::detect_available_space()` off the GTK thread; if
  `0 < available < needed`, sets the banner title
  ("Low disk space: X free, updates need about Y") and reveals it.

### Dead-code suppression

- Remove `#[allow(dead_code)]` from `Backend::estimate_size` and from the three
  now-live `disk.rs` helpers.

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| `estimate_size` runs an extra process per backend per check | It runs in the same off-thread task as `list_available`; the estimate commands are simulate/dry-run only (already used by item 11). |
| `df` blocking the GTK thread | Run via `spawn_background_async`, deliver result over a channel. |
| Backends without an estimate (pacman, nix, homebrew) | Default `estimate_size` returns `None`; the sum simply omits them and no suffix/banner is derived from missing data. |
| Estimate is download size vs. installed size, not exact | Labelled "~" / "about"; the banner is advisory, never blocks the update. |

## Success criteria

- Status label reads e.g. "12 updates available (~450 MB)" when at least one
  backend reports a size.
- `low_space_banner` reveals only when free space < estimated need.
- No `#[allow(dead_code)]` on `estimate_size` or the disk helpers.
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo build`,
  `cargo test` clean; `scripts/preflight.sh` exits 0.
