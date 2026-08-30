# PLUGIN_MANAGER — Review

Spec: `.github/docs/subagent_docs/PLUGIN_MANAGER_spec.md`
Scope: MASTER_PLAN item 23.

## Modified / new files

- `src/ui/preferences_dialog.rs` — **new**; `show_preferences_dialog`.
- `src/ui/mod.rs` — module registration.
- `src/ui/window.rs` — "Preferences" menu item + `win.preferences` action;
  `detect_backends` call passes `config.disabled_plugins`.
- `src/check.rs` — same call-site update.
- `src/backends/mod.rs` — `detect_backends(disabled_plugins: &[String])`;
  skips disabled descriptor ids.
- `src/config.rs` — `AppConfig::disabled_plugins`; round-trip test extended.
- `po/POTFILES.in` — new file listed.

## Findings

- **Part 1** confirmed already shipped (`meson.build`); no change. Examples
  stay examples.
- **Part 2** delivered: a real `adw::PreferencesDialog` (reusable by future
  items 25/45) with a Plugins group; per-descriptor `SwitchRow`; toggles
  persist to `disabled_plugins` and are honoured by `detect_backends` on next
  launch.
- `detect_backends` signature change contained to two call sites, both loading
  the config.
- Empty-state handled.
- Strings `gettext`-wrapped (dialog convention) and the file is in POTFILES.
- "Restart to apply" is stated in the group description — deliberate: live
  re-detection would rebuild every row.
- No new unit test for the `detect_backends` filter (needs on-disk descriptor
  fixtures); it's a one-line `iter().any()` and the config plumbing is
  round-trip tested.

## Build validation (`nix develop`)

| Command | Result |
|---|---|
| `cargo fmt --check` | pass |
| `cargo clippy -- -D warnings` | pass (0 warnings) |
| `cargo build` | pass |
| `cargo test` | 157 passed / 0 failed |
| `scripts/preflight.sh` | exit 0 |

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 100% | A |
| Best Practices | 96% | A |
| Functionality | 93% | A |
| Code Quality | 95% | A |
| Security | 100% | A |
| Performance | 96% | A |
| Consistency | 97% | A |
| Build Success | 100% | A |

**Overall Grade: A (97%)**

## Result

PASS
