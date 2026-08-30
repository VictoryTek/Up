# PLUGIN_MANAGER — Specification

MASTER_PLAN item 23 — ship plugin descriptors + add a Plugin manager UI.
Source: FEATURES 10. User decision: build Part 2.

## Part 1 — already done

`meson.build:63-67` already installs `data/backends.d/apk.yaml` and
`xbps.yaml` to `<datadir>/up/backends.d/`, so Alpine / Void users get plugin
support out of the box. `examples/plugins/{eopkg,swupd}.yaml` stay as examples
by design (not installed). No change needed.

## Part 2 — Plugin manager UI

### Config

`AppConfig` gains `disabled_plugins: Vec<String>` (`#[serde(default)]`) — the
descriptor ids the user has turned off.

### Detection

`backends::detect_backends()` → `detect_backends(disabled_plugins: &[String])`.
Discovered descriptors whose `id` is in the list are logged and skipped before
the binary-availability / builtin-duplicate checks. Both call sites
(`ui/window.rs`, `check.rs`) load the config and pass
`config.disabled_plugins`.

### UI

New `src/ui/preferences_dialog.rs::show_preferences_dialog(parent)`:

- `adw::PreferencesDialog` → `adw::PreferencesPage` ("General") →
  `adw::PreferencesGroup` ("Plugins", with a description noting changes apply
  after restart).
- One `adw::SwitchRow` per `discover_plugins()` descriptor (title =
  `display_name`, subtitle = `description`, `active = !disabled`).
- `connect_active_notify` reloads the config, adds/removes the id from
  `disabled_plugins`, and saves.
- Empty state: a single dimmed `ActionRow`.

Wired via a new `win.preferences` `SimpleAction` and a "Preferences" item in
the existing overflow menu (between "Clean Up" and "About Up").

Strings use `gettext` (matching the other dialogs); `preferences_dialog.rs`
added to `po/POTFILES.in`.

### Why "restart to apply"

Enabling/disabling a plugin changes which backends exist; live re-detection
would mean rebuilding every row mid-session. The switch persists immediately
and `detect_backends` honours it on the next launch — consistent effort/risk
for a v1 plugin manager.

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| `detect_backends` signature change | Only two call sites; both updated. |
| `discover_plugins` I/O on the GTK thread when opening the dialog | Bounded: a few directory reads + small YAML parses; acceptable for a modal open. |
| Config schema growth | `#[serde(default)]`; old/new config files interoperate. |

## Success criteria

- apk/xbps descriptors install (unchanged, verified).
- "Preferences" menu item opens a dialog listing discovered plugins with
  working enable switches; disabling one persists and it does not load next
  launch.
- Config round-trip test covers `disabled_plugins`.
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo build`,
  `cargo test` clean; `scripts/preflight.sh` exits 0.
