# Spec: Version Bump to 2.1.0

**Date:** 2026-07-09

## Current State

- Version: `2.0.4` in `Cargo.toml`, `daemon/Cargo.toml`, and
  `data/io.github.up.metainfo.xml`.
- Last tag: `v2.0.4`.
- Release notes file: `releases/2.0.4.md`.
- `flake.nix` reads the version dynamically from `Cargo.toml`
  (`(builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version`)
  — no separate edit needed there.
- `Cargo.lock` has its own `version = "2.0.4"` entry for the `up` package,
  regenerated automatically by `cargo build`/`cargo test` — no manual edit
  needed, but it must be checked in after those commands run so the repo
  stays consistent.
- No other file references the version string (checked `*.nix`, `*.md`,
  `*.toml`, `*.xml` outside `releases/` and `.github/docs/subagent_docs/`).

## Why 2.1.0 (minor) and not 2.0.5 (patch)

Commits since `v2.0.4` (`git log v2.0.4..HEAD`) are feature additions, not
bug fixes: a UI facelift, layout changes, and a new dialog/workflow. Per
semver convention already followed by this project (`1.1.0`, `2.0.0` were
used for feature releases; `2.0.1`–`2.0.4` were patch/fix releases), a
minor bump to `2.1.0` is correct.

## Changes Since 2.0.4 (commits since v2.0.4 tag)

| Hash | Message |
|------|---------|
| 510052d | feat: VexOS UI facelift with cyan/orange brand theme |
| a67428f | feat(ui): move buttons into hero row; cap terminal height |
| a4f0a19 | Update log_panel.rs (folded into a67428f's follow-up) |
| 6856fdc | feat(ui): open terminal panel expanded by default |
| 5317b6d | Update window.rs (trivial one-line follow-up) |
| ad405b8 | add: analysis (internal docs only, not user-facing) |
| 85cb83c | feat(ui): add cache-block dialog with just deploy/update-all bypass |

## Files Requiring Version Bump

1. `Cargo.toml` — workspace root, line 6: `version = "2.0.4"` → `"2.1.0"`
2. `daemon/Cargo.toml` — daemon crate, line 3: `version = "2.0.4"` →
   `"2.1.0"`
3. `data/io.github.up.metainfo.xml` — prepend new
   `<release version="2.1.0" date="2026-07-09">` entry above the existing
   `2.0.4` entry
4. `releases/2.1.0.md` — new release notes file (CREATE)
5. `Cargo.lock` — regenerated automatically by `cargo build` after the
   `Cargo.toml` edits; verified, not hand-edited

## Release Notes Content (2.1.0)

### New Features
- **VexOS cache-block dialog**: When a VexOS update is paused because
  kernel packages require a local source build the binary cache hasn't
  finished yet, Up now shows a dialog explaining exactly what's blocked
  and lets you choose `just deploy` (apply pending config without
  bumping nixpkgs), `just update-all` (force the local build now), or
  Wait (closes Up to retry once Hydra catches up).
- **VexOS brand theme**: New cyan/orange visual theme — navigation tabs
  moved into the header bar, a hero area with app icon and live status,
  and a themed Update All button, progress bar, and banners.

### Improvements
- **Layout**: Update All and Cancel buttons moved into the hero row,
  saving a full row of vertical space; the terminal output panel is
  capped at 200px when expanded so it no longer crowds the content above
  it.
- **Terminal panel**: Now expanded by default on launch so update output
  is immediately visible; default window height increased to
  accommodate it comfortably.

## Implementation Steps

1. Edit `Cargo.toml` line 6: `2.0.4` → `2.1.0`.
2. Edit `daemon/Cargo.toml` line 3: `2.0.4` → `2.1.0`.
3. Edit `data/io.github.up.metainfo.xml`: insert new `<release>` block
   (dated 2026-07-09) directly above the `2.0.4` entry, following the
   exact structure/indentation of existing entries.
4. Create `releases/2.1.0.md` with the content above, matching the
   heading/section style of `releases/2.0.4.md`.
5. Run `cargo build` so `Cargo.lock`'s `up` package version entry updates
   to `2.1.0` (verify via `grep -A2 'name = "up"' Cargo.lock`).
6. Build/test/lint validation (Phase 3), then preflight (Phase 6).

## Dependencies

None — no new external dependencies, no Context7 lookup required (pure
metadata/docs change).

## Configuration Changes

`data/io.github.up.metainfo.xml` changes must still pass AppStream
validation (`appstreamcli validate`) per the project's Repository Notes;
`appstreamcli` was not present in this environment during the prior
2.0.4 bump either (preflight skips it with a notice when absent), so this
is checked structurally against the existing entries' format instead.

## Risks and Mitigations

- **Risk:** New `<release>` XML block malformed, breaking
  `appstreamcli validate` in CI even though it's not checkable locally.
  **Mitigation:** Copy the exact structure of the adjacent `2.0.4` entry
  (same tag nesting, `translate="no"` attribute, closing tags) and only
  change the version/date/description text.
- **Risk:** Forgetting to update `Cargo.lock`, causing a mismatch CI might
  flag. **Mitigation:** Explicit build step in the implementation steps to
  regenerate it, plus a verification grep.
