# Spec: Version Bump to 2.3.0

**Date:** 2026-08-22

## Current State

- Version: `2.2.0` in `Cargo.toml` (line 6). `daemon/Cargo.toml` no
  longer exists — the daemon crate was removed as part of this release
  (see below), so there is only one version string to bump this time,
  unlike the 2.2.0 bump which also touched `daemon/Cargo.toml`.
- Last tag/release entry: `2.2.0` (2026-07-11), in
  `data/io.github.up.metainfo.xml`'s `<releases>` list and
  `releases/2.2.0.md`.
- `flake.nix` reads the version dynamically from `Cargo.toml`
  (`(builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version`)
  — no separate edit needed, confirmed same as the 2.2.0 and 2.1.0 bumps.
- `Cargo.lock` has its own `version = "2.2.0"` entry for the `up`
  package; regenerated automatically by `cargo build`, not hand-edited.
  (No `up-daemon` entry to update — that package no longer exists in the
  workspace.)

## Scope: changes since the 2.2.0 tag

Per `git log --oneline b1efd4a..HEAD` (`b1efd4a` = the 2.2.0 bump
commit), the following are included in this release:

1. `489f5fe` — **fix(ui): reverse suffix order in Sources rows** — cosmetic
   reorder of the popover button / status / spinner / retry / skip
   checkbox suffix widgets in each Sources row.
2. `55fe5eb` — **fix(ui): reset updating flag when auth prompt fails** —
   the `AuthFailed` arm in the Update All event loop left `updating`
   stuck `true` on a cancelled/failed polkit prompt, permanently
   disabling Refresh and all per-row Retry buttons until restart.
3. (bundled into `782bc1a`, see note below) — **fix: `up --check` CLI now
   works** — `main()` never inspected `argv`, so the daily systemd
   timer's `up --check` was rejected by GTK/GApplication every time.
4. (bundled into `782bc1a`) — **fix: plugin backends needing root now
   actually run privileged** — `needs_root: true` plugin backends
   (Alpine/apk, Void/xbps, and example descriptors) prompted for admin
   auth but then executed unprivileged, failing every update.
5. (bundled into `782bc1a`) — **chore: removed the unused D-Bus daemon**
   — a fully-built but never-connected privileged D-Bus service (its own
   crate, systemd unit, D-Bus policy, polkit actions) was deleted; the
   GUI has always used `pkexec` directly. Internal-only, no user-facing
   behavior change — worth a brief release-notes mention for
   transparency (smaller installed footprint, one less root-privileged
   background service) but not a feature or fix from the user's
   perspective.
6. `782bc1a` — **feat: Update History page wired up** — a new "History"
   tab records and displays past update sessions (success/error/skip per
   backend, with counts and timestamps).
7. `f36bb64` — **feat: skip-backend preferences now persist** — the
   per-backend "skip during Update All" checkboxes now survive a
   restart instead of resetting every launch.
8. `66b03fc` — **feat: Cleanup / maintenance mode** — new "Clean Up" menu
   entry runs each backend's cleanup/maintenance operation (e.g. `apt
   autoremove`, `nix-collect-garbage`).
9. `9000480` — **feat: per-package selective updates** — the package
   list popover now has a checkbox per package (where the backend
   supports it), letting you update a subset instead of everything.

Note on commit boundaries: items 3-5 above have no standalone commit —
they were committed together with item 6 under `782bc1a`'s message
(confirmed via `git show --stat 782bc1a`, which touches
`src/main.rs`/`src/check.rs` (item 3), `src/plugins/backend.rs`/
`src/executor.rs` (item 4), and the `daemon/`/packaging deletions (item
5) alongside the History-page files). This doesn't change what's in the
release, only how it's organized in `git log`; release notes are written
from the actual diff contents, not commit-message boundaries.

## Why 2.3.0 (per explicit user instruction)

User explicitly requested `2.3.0`. Consistent with this project's semver
usage (minor bump for user-facing feature additions) — four new
user-visible features (History page, persisted preferences, Cleanup
mode, selective updates) plus three bug fixes clearly warrant a minor
bump over a patch bump.

## Files Requiring Version Bump

1. `Cargo.toml` — line 6: `version = "2.2.0"` → `"2.3.0"`.
2. `data/io.github.up.metainfo.xml` — prepend new
   `<release version="2.3.0" date="2026-08-22">` entry above the existing
   `2.2.0` entry, matching existing structure (`translate="no"`, single
   `<p>` description).
3. `releases/2.3.0.md` — new release notes file (CREATE), matching the
   heading/section style of `releases/2.1.0.md` and `releases/2.0.4.md`
   (`## What's Changed`, then `### New Features`, `### Improvements`,
   `### Bug Fixes` as applicable).
4. `Cargo.lock` — regenerated automatically by `cargo build`; verified
   via `grep -A2 'name = "up"' Cargo.lock`, not hand-edited.

No `daemon/Cargo.toml` edit this time — that file no longer exists.

## Release Notes Content (2.3.0)

```markdown
## What's Changed

### New Features
- **Update History**: A new History tab records every update session —
  which backends ran, how many packages were updated, and any errors —
  so you can look back at what happened on a previous run.
- **Selective updates**: The package list popover now shows a checkbox
  per package (where the backend supports it), so you can update just
  the packages you want instead of everything a backend has pending.
- **Cleanup / maintenance mode**: A new "Clean Up" entry in the menu runs
  each backend's cleanup operation (e.g. removing unused packages,
  garbage-collecting old Nix generations) in one click.
- **Persisted skip preferences**: Backends you've checked "skip during
  Update All" for now stay skipped after restarting Up, instead of
  resetting every launch.

### Bug Fixes
- **Update All getting stuck**: Cancelling or failing the admin
  authentication prompt during an update no longer leaves the Refresh
  and Retry buttons permanently disabled until restart.
- **`up --check`**: The daily background update check (run by the
  systemd timer) now actually runs instead of failing every time with an
  unrecognized-option error.
- **Plugin backends requiring root**: Plugin-based backends that need
  administrator privileges (e.g. Alpine/apk, Void/xbps) now actually run
  with those privileges after the authentication prompt, instead of
  failing every update.
- **Sources row layout**: Reordered the package-count, status, and
  action controls in each source row for a more natural reading order.

### Internal
- Removed an unused, never-connected D-Bus privileged service that
  shipped alongside the app — Up has always talked to the system
  directly via `pkexec`, so this reduces the app's installed footprint
  and removes an unreachable root-privileged background service with no
  change in behavior.
```

## Implementation Steps

1. Edit `Cargo.toml` line 6: `2.2.0` → `2.3.0`.
2. Edit `data/io.github.up.metainfo.xml`: insert new `<release>` block
   (dated 2026-08-22) directly above the `2.2.0` entry, with a
   single-paragraph summary condensed from the release notes above
   (AppStream release descriptions are conventionally one short
   paragraph, not the full itemized changelog — matching every existing
   entry in the file).
3. Create `releases/2.3.0.md` with the content above.
4. Run `cargo build` (inside `nix develop`) so `Cargo.lock`'s `up` entry
   updates to `2.3.0`; verify via grep.
5. Build/lint/test validation (Phase 3), then preflight (Phase 6).

## Dependencies

None — pure metadata/docs change, no Context7 lookup required.

## Configuration Changes

`data/io.github.up.metainfo.xml` changes must structurally match
existing `<release>` entries so `appstreamcli validate` (CI-enforced)
would pass; `appstreamcli` is not installed in this local environment
(preflight skips it with a notice, consistent with prior bumps).

## Risks and Mitigations

- **Risk:** Malformed new `<release>` XML block breaks AppStream
  validation in CI even though it isn't checkable locally.
  **Mitigation:** copy the exact structure of the adjacent `2.2.0` entry.
- **Risk:** `Cargo.lock` left stale. **Mitigation:** explicit `cargo
  build` + grep verification step.
- **Risk:** Release notes mention internal-only changes (daemon removal)
  in a way that confuses end users who never knew it existed.
  **Mitigation:** framed as a footprint/attack-surface reduction with "no
  change in behavior" stated explicitly, under a separate "Internal"
  heading rather than mixed into user-facing fixes/features.
