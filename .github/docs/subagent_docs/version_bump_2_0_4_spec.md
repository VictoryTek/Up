# Spec: Version Bump to 2.0.4

## Current State
- Version: `2.0.3` in `Cargo.toml`, `daemon/Cargo.toml`, and `data/io.github.up.metainfo.xml`
- Last tag: `v2.0.3`
- Release notes file: `releases/2.0.3.md`

## Changes Since 2.0.3 (commits since v2.0.3 tag)

| Hash     | Message |
|----------|---------|
| f9f022e  | fix(backends): fix NixOS/VexOS false positive and Flatpak false negative |
| a9bba14  | create: analysis |
| 99477ba  | fix: move update button to fixed footer; add cancel; fix VexOS check |
| 447d996  | Update parser.rs |
| 91a5c31  | Update flatpak.rs |

## Files Requiring Version Bump

1. `Cargo.toml` — workspace root, line 6: `version = "2.0.3"` → `"2.0.4"`
2. `daemon/Cargo.toml` — daemon crate, line 3: `version = "2.0.3"` → `"2.0.4"`
3. `data/io.github.up.metainfo.xml` — prepend new `<release version="2.0.4" date="2026-06-14">` entry
4. `releases/2.0.4.md` — new release notes file (CREATE)

## Release Notes Content (2.0.4)

### Bug Fixes
- **NixOS/VexOS**: Fixed false positive update detection on NixOS and VexOS — the check no longer incorrectly reports updates available when there are none.
- **Flatpak**: Fixed false negative update detection — the check now correctly identifies available Flatpak updates.
- **VexOS**: Fixed VexOS update command check producing incorrect results.

### Improvements
- **UI**: Update button moved to fixed footer for consistent accessibility; Cancel button added to allow aborting in-progress updates.

## Risks
- `Cargo.lock` will update automatically on next `cargo build` to reflect the new version — this is expected and correct.
- AppStream metainfo must pass `appstreamcli validate --no-net` after adding the new release entry.
