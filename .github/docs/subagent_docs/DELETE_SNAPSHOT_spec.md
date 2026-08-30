# DELETE_SNAPSHOT — Specification

MASTER_PLAN item 18 — wire up or delete the snapshot subsystem.
User decision (2026-08-29): **delete**.

## What was removed

- `src/snapshot.rs` (whole file, 119 lines) — `SnapshotTool`, `SnapshotError`,
  `detect_snapshot_tool()`, `is_root_btrfs()`, `create_snapshot()`. Zero
  callers; the `#[allow(dead_code)]`-suppressed module had been dead since it
  was written.
- `mod snapshot;` in `src/main.rs`.
- `src/config.rs::SnapshotPreference` enum and the
  `AppConfig::snapshot_preference` field (both only ever referenced by the
  config round-trip test).
- The two `snapshot_preference` lines in the config round-trip test.

## Not touched

- `data/io.github.up.metainfo.xml:76` — the 2.x `<release>` note still lists
  "pre-update snapshots" as a shipped feature. It was already inaccurate (the
  code never had a caller). Retroactively editing a published release
  description is out of scope here; fold it into MASTER_PLAN item 47 (README /
  metadata feature-matrix cleanup).

## Compatibility

`AppConfig` is `#[serde(default)]` per field and `serde` ignores unknown keys,
so an existing `~/.config/up/config.json` that still contains
`"snapshot_preference": …` loads fine — the key is simply dropped on the next
save.

## Success criteria

- No `snapshot` / `Snapshot` symbol anywhere in `src/`.
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo build`,
  `cargo test` clean; `scripts/preflight.sh` exits 0.
