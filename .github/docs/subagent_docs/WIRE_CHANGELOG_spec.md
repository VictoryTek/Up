# WIRE_CHANGELOG — Specification

MASTER_PLAN item 20 — `changelog.rs` fully implemented, zero callers.
Source: ARCH M12, BUGS M3, FEATURES 8. User decision: wire up.

## Problem

`src/changelog.rs` (per-backend changelog / release-notes fetchers for
apt/dnf/pacman/zypper/flatpak/homebrew/fwupd, 30 s timeout, output cap) is
behind `#![allow(dead_code)]` with no caller. Users see "N updates available"
with no way to know what changed.

## Design

### `src/changelog.rs`

- Remove `#![allow(dead_code)]`.
- Add `pub fn supports_changelog(kind: &BackendKind) -> bool` — `false` for
  `Nix` and `Plugin(_)`, `true` otherwise (mirrors the `fetch_changelog`
  match).
- `run_cmd` keeps its direct `tokio::process::Command` (it needs the
  `tokio::time::timeout` wrapper, which the `CommandExecutor::probe`
  abstraction does not provide).

### `src/ui/update_row.rs`

- `UpdateRow` gains `kind: BackendKind`, a `changelog_button: gtk::Button`
  (icon `document-properties-symbolic`, tooltip "What's new", flat), and
  `packages: Rc<RefCell<Vec<String>>>`.
- `set_packages` records the package list.
- Button visibility: shown by `set_status_available` when `count > 0 &&
  supports_changelog(kind)`; hidden by `set_status_checking` /
  `set_status_running` / `set_status_unknown` and toggled by the skip
  checkbox.
- New free fn `show_changelog_dialog(parent, kind, packages)`: an
  `adw::AlertDialog` ("What's New") whose `extra_child` is a non-editable
  monospace `TextView` in a `ScrolledWindow` (480×320). Fetches
  `fetch_changelog` via `spawn_background_async` + channel, fills the buffer
  on the GTK thread; empty result → "No changelog information available.",
  error → "Could not fetch changelog:\n<e>".

No `window.rs` change needed — the row owns its button, kind, and package list.

## Strings

New literal strings ("What's New", "Close", "Fetching changelog…", …) are left
un-`gettext`-wrapped to match the rest of `update_row.rs` / `window.rs`;
localisation is MASTER_PLAN item 25.

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| `fetch_changelog` hangs | Existing 30 s `timeout` per command in `run_cmd`. |
| Large output | Existing 10 000-char cap (`truncate`). |
| Dialog opened for a backend with no changelog support | Button only shown when `supports_changelog(kind)`. |
| Package list capped at 50 in the popover | The changelog list is stored uncapped in `packages`. |

## Success criteria

- A "What's new" button appears on rows with pending updates for
  apt/dnf/pacman/zypper/flatpak/homebrew/fwupd; clicking it shows fetched
  changelog text (or a clean fallback message).
- `changelog.rs` has no `#![allow(dead_code)]` — completes MASTER_PLAN
  item 15 (all 7 modules resolved).
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo build`,
  `cargo test` clean; `scripts/preflight.sh` exits 0.
