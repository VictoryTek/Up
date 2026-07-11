# Spec: Version Bump to 2.2.0

**Date:** 2026-07-11

## Current State

- Version: `2.1.0` in `Cargo.toml`, `daemon/Cargo.toml`, and
  `data/io.github.up.metainfo.xml`.
- Last tag/release entry: `2.1.0` (2026-07-09).
- Release notes file: `releases/2.1.0.md`.
- `flake.nix` reads the version dynamically from `Cargo.toml`
  (`(builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version`)
  — no separate edit needed there (confirmed same as prior 2.1.0 bump).
- `Cargo.lock` has its own `version = "2.1.0"` entry for the `up` package,
  regenerated automatically by `cargo build` — no manual edit, but must be
  verified after the `Cargo.toml` edit.
- `src/changelog.rs` is unrelated (fetches upstream package changelogs, not
  the app's own version) — no changes needed there.

## Scope: "Last 2 Changes"

Per user request, this release covers the two most recent changes on top of
the 2.1.0 tag:

1. **`c6495c3` — fix(ui): show real updated items instead of vague/mismatched
   dropdown** (already committed).
2. **Package-list popover redesign** (this session, currently uncommitted in
   the working tree: `src/ui/update_row.rs`, `data/style.css`) — replaces the
   `Adw.ExpanderRow` inline package list with a `Gtk.Popover` opened from a
   per-row "N pkgs" button, so viewing updated packages no longer grows the
   page or triggers the page-level scrollbar.

## Why 2.2.0 (per explicit user instruction)

User explicitly requested `2.2.0`. Consistent with this project's semver
usage (minor bumps for user-facing feature/UX changes, patch bumps for
narrow bug fixes) — the popover redesign is a visible UX change, so a minor
bump is appropriate regardless.

## Files Requiring Version Bump

1. `Cargo.toml` — line 6: `version = "2.1.0"` → `"2.2.0"`
2. `daemon/Cargo.toml` — line 3: `version = "2.1.0"` → `"2.2.0"`
3. `data/io.github.up.metainfo.xml` — prepend new
   `<release version="2.2.0" date="2026-07-11">` entry above the existing
   `2.1.0` entry, matching existing structure (`translate="no"` description).
4. `releases/2.2.0.md` — new release notes file (CREATE), matching the
   heading/section style of `releases/2.1.0.md`.
5. `Cargo.lock` — regenerated automatically by `cargo build`; verified via
   `grep -A2 'name = "up"' Cargo.lock`, not hand-edited.

## Release Notes Content (2.2.0)

```markdown
## What's Changed

### Improvements
- **Package list popover**: Viewing a backend's updated packages no longer
  expands the row inline and pushes the whole page into a scrollbar. Each
  source row now shows a package-count button that opens a small popover
  with the full list, so the main window never grows or scrolls to show it.
- **Accurate updated-items list**: The list of updated packages shown after
  a run now reflects what was actually updated, instead of a placeholder or
  mismatched set of items.
```

## Implementation Steps

1. Edit `Cargo.toml` line 6: `2.1.0` → `2.2.0`.
2. Edit `daemon/Cargo.toml` line 3: `2.1.0` → `2.2.0`.
3. Edit `data/io.github.up.metainfo.xml`: insert new `<release>` block
   (dated 2026-07-11) directly above the `2.1.0` entry.
4. Create `releases/2.2.0.md` with the content above.
5. Run `cargo build` (inside `nix develop`, per project constraints) so
   `Cargo.lock`'s `up` entry updates to `2.2.0`; verify via grep.
6. Build/lint/test validation (Phase 3), then preflight (Phase 6).

## Dependencies

None — pure metadata/docs change, no Context7 lookup required.

## Configuration Changes

`data/io.github.up.metainfo.xml` changes must still structurally match
existing `<release>` entries so `appstreamcli validate` (CI-enforced) would
pass; `appstreamcli` is not installed in this local environment (preflight
skips it with a notice, consistent with the 2.1.0 bump).

## Risks and Mitigations

- **Risk:** Malformed new `<release>` XML block breaks AppStream validation
  in CI even though it isn't checkable locally.
  **Mitigation:** Copy the exact structure of the adjacent `2.1.0` entry.
- **Risk:** `Cargo.lock` left stale.
  **Mitigation:** Explicit `cargo build` + grep verification step.
- **Risk:** The uncommitted popover changes from this session are not yet
  reviewed/preflighted as part of *this* version bump's own validation pass.
  **Mitigation:** Re-run the full preflight suite (fmt, clippy, build,
  daemon build, tests, nix flake check) after the version bump edits, so the
  final preflight covers both the popover change and the bump together.
