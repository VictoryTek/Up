# Up — Master Plan

Consolidated from `ANALYSIS_ARCH.md`, `ANALYSIS_BUGS.md`, `ANALYSIS_FEATURES.md`.
Duplicate findings (same underlying issue reported in multiple docs) have been
merged into a single item; each item lists every source doc that flagged it.
Ordered: High → Medium → Low. Within High, ordered roughly by
fix-it-now-ness (small safe bug fixes first, architectural decisions next,
feature-wiring last).

Checklist convention: `[ ]` open, `[x]` done.

---

## HIGH PRIORITY

- [x] **1. `AuthFailed` event leaves `updating` flag stuck `true`, wedging the UI**
  Source: BUGS H2.
  Files: `src/ui/window.rs:512-521`.
  The `AuthFailed` arm in the "Update All" event loop `return`s without calling
  `updating.set(false)`. Cancelling/failing the polkit prompt permanently
  disables the Refresh button and all per-row Retry buttons until restart.

- [x] **2. `up --check` is completely broken — no CLI argument handling in `main()`**
  Source: ARCH H2, BUGS H1, FEATURES 1.
  Files: `src/main.rs:23-28`, `src/check.rs` (whole file, dead), `data/io.github.up-check.service.in`.
  `main()` never inspects `std::env::args()`; the daily systemd timer's
  `up --check` is handed straight to GTK/GApplication, which rejects the
  unknown option and fails every day. `check.rs` is a complete, unused
  implementation (stamp file, notify-send, count aggregation). Note:
  `check::run_check()` also calls `env_logger::init()` a second time — must
  reconcile with `main.rs`'s existing call before wiring in.

- [x] **3. Plugin backends with `needs_root: true` authenticate but then run unprivileged**
  Source: ARCH H3.
  Files: `src/plugins/backend.rs:53-75`, `src/runner.rs:299-307`, `src/orchestrator.rs:96-117`, `data/backends.d/apk.yaml`, `data/backends.d/xbps.yaml`.
  `CommandRunner` only routes through the elevated shell when the program
  string is literally `"pkexec"`. Plugin backends prompt the user for admin
  auth, then execute the actual command directly as the unprivileged user —
  the update always fails with a permissions error after wasting an auth
  prompt.

- [x] **4. Decide the fate of the D-Bus daemon: wire it in, or remove it**
  Source: ARCH H1 & M5, BUGS M1, FEATURES 9.
  Files: `daemon/` (whole crate), `src/dbus_client.rs` (not even in the module
  tree — not compiled), `src/main.rs:1-17`, `data/io.github.up.Daemon.*`,
  `data/io.github.up.policy`.
  A full polkit-authenticated D-Bus service (allowlist, audit log,
  cancellation, idle lifecycle) is built and installed as a live root-callable
  systemd/D-Bus service, but the GUI never connects to it — it uses `pkexec`
  directly instead. The daemon's own allowlist has already diverged from what
  the GUI actually runs (Nix, pacman cleanup), and `run_upgrade`'s command
  table is empty so that D-Bus method can never succeed. This is a decision
  that needs to be made before several other items (item 3's fix, item 7
  below) can be finalized cleanly.
  **This needs a decision from you** — wire in (real mid-command cancel,
  smaller root attack surface, one polkit prompt per session) or delete
  (daemon crate + packaging + policy, ~1 day). Blocks item 7.

- [x] **5. Remove unused dependencies `zbus`, `futures-util`, `tokio-util` from root crate**
  Source: ARCH H6.
  Files: `Cargo.toml:30-32`.
  Only used by `src/dbus_client.rs`, which isn't compiled. Depends on the
  outcome of item 4 — skip/redo if the daemon gets wired in instead.

- [x] **6. Wire up the Update History page**
  Source: ARCH H5, BUGS M2, FEATURES 2.
  Files: `src/history.rs`, `src/ui/history_page.rs`, `src/ui/window.rs:32-52` (ViewStack), `BackendFinished` handlers at `window.rs:442-568` and `:791-911`.
  Storage layer and UI page are both fully built and dead. Needs: (a) add a
  third ViewStack page, (b) call `history::append_entry()` from the
  `BackendFinished` arms in both the Update-All loop and the retry loop.

- [x] **7. Persist user preferences (skip-backend choices) across restarts**
  Source: FEATURES 3 (related dead code also noted in ARCH M9, BUGS M4).
  Files: `src/config.rs` (dead, zero callers), `src/ui/update_row.rs` (skip checkboxes, session-only).
  `AppConfig` with JSON load/save already exists. Needs: load config on
  backend-detection completion and pre-set checkboxes; save on
  `on_skip_changed`.

- [x] **8. Wire up Cleanup / maintenance mode**
  Source: ARCH M11, BUGS M4, FEATURES 4.
  Files: `src/orchestrator.rs:207-274` (`CleanupOrchestrator`, dead), every backend's `run_cleanup()`/`supports_cleanup()`, `src/ui/window.rs` (no entry point).
  Every backend already implements real cleanup logic (`apt autoremove`,
  `nix-collect-garbage -d`, etc.) and a finished orchestrator reuses the
  existing event/auth/log pipeline — there is simply no button. Add a "Clean
  Up" menu entry that drives `CleanupOrchestrator::run_all()`.

- [x] **9. Make per-package selective updates real (checkboxes in the UI)**
  Source: ARCH H4, FEATURES 7.
  Files: `src/backends/mod.rs:196-223`, `src/ui/window.rs:409` (always passes `None`), `src/ui/update_row.rs:124-154`.
  Full backend + orchestrator plumbing for selecting a subset of packages
  exists (with per-backend name validation) but the UI never lets the user
  pick — it always passes `None`. Add checkboxes to `UpdateRow`'s package
  list and thread `selected_items()` through to the Update-All handler.
  Care needed around the existing 50-item display cap. Depends on item 4 if
  daemon adoption changes how selection is dispatched.

---

## MEDIUM PRIORITY

- [x] **10. VexOS vendor coupling hard-wired into the generic Nix backend** *(partial)*
  Source: ARCH M1.
  Files: `src/backends/nix.rs`.
  Flake-attr resolution now falls back to auto-detecting the
  `nixosConfigurations` attribute (single config, or hostname match) via
  `nix eval` when `/etc/nixos/vexos-variant` is absent — plain flake NixOS
  users no longer hit the VexOS-only error. `UpdateResult::CacheMiss` is
  genuinely VexOS-specific and threaded through the whole UI; decoupling it
  was deliberately descoped (user decision, 2026-08-29) and remains open as
  future cleanup if desired.

- [x] **11. Read-only backend operations bypass the `CommandExecutor` abstraction** *(partial)*
  Source: ARCH M2.
  Files: `src/executor.rs`, `src/runner.rs`, `src/backends/mod.rs`, `flatpak.rs`, `fwupd.rs`, `homebrew.rs`, `os_package_manager.rs`, `nix.rs`, `src/plugins/backend.rs`, `src/check.rs`, `src/ui/window.rs`.
  Added `CommandExecutor::probe()` + `ProbeOutput` + a non-streaming
  `SystemExecutor`; `list_available` / `estimate_size` / `count_available` now
  take a `runner` and route every read-only spawn through it (except
  `nixos_flake_tempdir_check`, which needs `current_dir` + fs copies —
  documented). These paths are now `MockExecutor`-testable (13 new tests).
  Still open (descoped, user decision 2026-08-29): sync detection probes
  (`is_nixos`/`os_package_manager::detect`/… — need a sync `SystemProber`),
  and the `nix profile upgrade` / `nix-env` streaming-during-update gap (M2c).

- [x] **12. Unify the two privileged-execution stacks (update vs. upgrade)**
  Source: ARCH M3.
  Files: `src/upgrade/execute.rs`, `src/runner.rs`, `src/upgrade/mod.rs`, `src/ui/upgrade_page.rs`.
  `execute_upgrade` is now async and runs every step through a shared
  `PrivilegedShell`-backed `CommandRunner` (new `run_upgrade()` entry point).
  One polkit prompt per upgrade (Fedora: 4→1, NixOS: 2→1). `run_command_sync`
  (the blocking re-implementation of `CommandRunner::run`) deleted. Upgrade
  paths still lack integration tests — manual verification on real distros
  advised before shipping.

- [x] **13. Replace stringly-typed contracts between layers** *(partial — part A)*
  Source: ARCH M4.
  Files: `src/upgrade/version.rs`, `src/upgrade/mod.rs`, `src/ui/upgrade_page.rs`.
  `check_upgrade_available` now returns an `UpgradeAvailability` enum; the UI
  gates on `is_available()` instead of `result_msg.starts_with("Yes")`.
  Still open (descoped, user decision 2026-08-29): part B — typed
  `PrivilegedShell` / `BackendError` (kill `BackendError::from_string` prose
  parsing in `src/backends/mod.rs:42-69`, `src/runner.rs`); part C —
  `history.rs` `result: String` → enum.

- [x] **14. Daemon allowlist diverged from GUI commands / `RunUpgrade` can never succeed** *(moot — daemon removed)*
  Source: ARCH M5 (also see item 4 — resolve together).
  Files: ~~`daemon/src/allowlist.rs`~~ (deleted).
  Resolved by item 4: the `daemon/` crate, its D-Bus/systemd data files, and
  `src/dbus_client.rs` were removed; `data/io.github.up.policy` now contains
  only the pkexec actions the GUI actually uses. Nothing to reconcile.
  Verified 2026-08-29: `daemon/` does not exist; `Cargo.toml` workspace
  `members = ["."]`. This also moots items 34, 37, 41, 44 (all daemon-only).

- [x] **15. Remove blanket `#![allow(dead_code)]` from the 7 abandoned-subsystem modules**
  Source: ARCH M6.
  All 7 resolved: `check.rs`/`config.rs`/`history.rs`/`ui/history_page.rs`
  (items 2/6/7), `disk.rs` (items 11/19 — now fully live),
  `snapshot.rs` (deleted, item 18), `changelog.rs` (wired up, item 20). No
  module-wide `#![allow(dead_code)]` remains anywhere; only a few targeted
  per-symbol allows persist (e.g. unused `BackendError` variants).

- [x] **16. Orchestrator event loop duplicated in the UI with behavioral drift** *(partial)*
  Source: ARCH M7.
  Files: `src/ui/window.rs`.
  Extracted `apply_backend_finished()` — the row-status / VexOS cache-dialog /
  history handling for a `BackendFinished` event, shared by the "Update All"
  loop and the retry loop (a new `UpdateResult` variant now needs handling in
  one place). Fixed the retry self-update-banner drift. Still open: the retry
  path still has no progress bar and drops the `CancelHandle` (deliberate —
  single-backend quick action, adding a cancel button is a UX decision).

- [x] **17. `upgrade_supported` and `execute_upgrade` disagree about supported distros**
  Source: ARCH M8.
  Files: `src/upgrade/detect.rs`, `src/upgrade/execute.rs`, `src/upgrade/version.rs`, `src/upgrade/mod.rs`.
  New `UpgradeStrategy::for_distro()` in `detect.rs` is the single source of
  truth; `upgrade_supported`, `execute_upgrade`, `check_upgrade_available`,
  and `upgrade_kind` all derive from it. Distros with no implemented path
  (debian, mint, pop, elementary, zorin, rhel, centos, ID_LIKE matches) are
  no longer falsely reported as supported.

- [x] **18. Wire up or delete the snapshot subsystem (Timeshift/Snapper/btrfs)** *(deleted — user decision 2026-08-29)*
  Source: ARCH M9, BUGS M4, FEATURES 5.
  Removed `src/snapshot.rs`, `src/config.rs::SnapshotPreference` + the
  `AppConfig::snapshot_preference` field, and `mod snapshot;`. The daemon copy
  was already gone with item 4. Metainfo release note still mentions
  snapshots → folded into item 47. Also advances item 15 (6 of 7 modules
  de-suppressed).

- [x] **19. Wire up disk-size estimation in the update rows**
  Source: ARCH M10, BUGS M3, FEATURES 6.
  Files: `src/ui/window.rs`, `src/ui/update_row.rs`, `src/backends/mod.rs`, `src/disk.rs`.
  The availability check now also calls `estimate_size()`; the status label
  shows "N updates available (~450 MB)" and a `low_space_banner` reveals when
  free space on `/` is below the estimated need. `estimate_size` and the
  `disk.rs` helpers are de-suppressed (finishes item 15 — only `changelog.rs`
  left, pending item 20).

- [x] **20. `changelog.rs` is fully implemented but has zero callers**
  Source: ARCH M12, BUGS M3, FEATURES 8.
  Files: `src/changelog.rs`, `src/ui/update_row.rs`.
  Added a per-row "What's new" button (apt/dnf/pacman/zypper/flatpak/homebrew/
  fwupd) that fetches `fetch_changelog()` off-thread into a scrollable dialog;
  `supports_changelog()` gates visibility. Completes item 15 (last
  `#![allow(dead_code)]` module).

- [x] **21. Replace `serde_yml 0.0.12` with a maintained YAML parser**
  Source: ARCH M13.
  Files: `Cargo.toml`, `Cargo.lock`, `src/plugins/discovery.rs`, `src/plugins/descriptor.rs`.
  Replaced with `yaml_serde 0.10` — the YAML-Organization continuation of
  `serde_yaml` (actively released, `from_str`-compatible). Drop-in; new
  `shipped_descriptors_parse` test covers the real descriptor format.
  (`serde_yaml_ng`'s last release was ~2 yrs old; `yaml_serde` is current.)

- [x] **22. Fix package-count miscounting for APT selective updates and DNF/generic prerequisite checks**
  Source: BUGS M5 & M6 (also ARCH L7 — same DNF issue reported twice).
  Files: `src/backends/os_package_manager.rs`, `src/upgrade/check.rs`.
  M6: `check_packages_up_to_date` now counts via the backend parsers
  (`parse_dnf_list_upgrades` etc.), so a clean Fedora's metadata header no
  longer blocks the upgrade. M5: APT update/selective-update commands run
  under `LC_ALL=C`; `count_apt_upgraded` uses a strict summary match with a
  dpkg "Setting up" fallback for the "0 upgraded" case.

- [ ] **23. Ship the existing plugin descriptors + add a Plugin manager UI**
  Source: FEATURES 10.
  Files: `data/backends.d/apk.yaml`, `xbps.yaml`, `examples/plugins/eopkg.yaml`, `swupd.yaml`.
  Install the shipped descriptors via meson so Alpine/Void users get support
  out of the box; add a preferences-dialog section listing/toggling
  discovered plugins.

- [ ] **24. Show error tail on click instead of a truncated one-line label**
  Source: FEATURES 11.
  Files: `src/runner.rs` (`tail_str` already retained, discarded on error path), `src/ui/update_row.rs` (`set_status_error`).
  Include the retained 100-line output tail in `BackendError::Exit::message`
  and make the error label open a dialog with full context.

- [ ] **25. Finish localization: initialize gettext and wrap remaining UI strings**
  Source: ARCH L4, FEATURES 14.
  Files: `src/main.rs` (no `bindtextdomain`/`textdomain` call anywhere), `src/ui/window.rs`, `update_row.rs`, `log_panel.rs` (raw string literals despite being listed in `po/POTFILES.in`).
  The translation infrastructure (po/, meson i18n merge, gettext-rs dep) is
  fully present and fully non-functional without this.

- [ ] **26. Flatpak packaging**
  Source: FEATURES 15.
  Files: sandbox plumbing already exists (`flatpak-spawn --host` routing, `is_running_in_flatpak()`, `SuccessWithSelfUpdate` restart banner) with no consumer; README says "planned for a future release."
  This is packaging work (Flathub manifest, `--talk-name=org.freedesktop.Flatpak`), not app code.

---

## LOW PRIORITY

- [ ] **27. Duplicate spawn helpers with identical bodies and stale docs**
  Source: ARCH L1. Files: `src/orchestrator.rs:197-205`, `src/ui/mod.rs:10-22`.

- [ ] **28. Colliding module names across crates/trees (`executor`, `check`)**
  Source: ARCH L2. Files: `src/executor.rs` vs `daemon/src/executor.rs`; `src/check.rs` vs `src/upgrade/check.rs`.

- [ ] **29. Five divergent inline package-name validators + two flake-attr validators**
  Source: ARCH L3. Files: `os_package_manager.rs` (APT/DNF/Zypper, each different), `homebrew.rs`, `nix.rs::validate_flake_attr`, `upgrade/version.rs::validate_hostname` (dead duplicate).

- [ ] **30. Hardcoded plugin/builtin alias table in `detect_backends()`**
  Source: ARCH L5. Files: `src/backends/mod.rs:266-281`.

- [ ] **31. Generated `.desktop` file committed alongside its `.in` source**
  Source: ARCH L6. Files: `data/io.github.up.desktop`, `data/io.github.up.desktop.in`.

- [ ] **32. Mixed `pkexec` invocation styles across backends (`sh -c` vs argv vs `env`)**
  Source: ARCH L8. Files: `os_package_manager.rs`, `nix.rs`, `upgrade/execute.rs`.

- [ ] **33. Three inconsistent log-channel/stderr-prefix conventions**
  Source: ARCH L9. Files: `src/runner.rs:468` (`"stderr: "`), `src/upgrade/execute.rs:192` (`"[stderr] "`), `CommandRunner` (no marker at all).

- [x] **34. Daemon operation-cleanup poll loop copy-pasted four times; idle timeout hardcoded twice** *(moot — daemon removed per item 4)*
  Source: ARCH L10. Files: ~~`daemon/`~~ (deleted).

- [ ] **35. Misc vestiges: dead flags, decorative `min_up_version` check, inverted "legacy" polkit comment**
  Source: ARCH L11. Files: `src/backends/flatpak.rs:100`, `src/orchestrator.rs:12,18`, `src/plugins/validate.rs:97-101`, `daemon/src/allowlist.rs:166-181`, `data/io.github.up.policy`.

- [ ] **36. Minor dependency cleanups: duplicate `glib`/`gio` sourcing, per-call regex recompilation, `ureq` as the lone blocking-HTTP island**
  Source: ARCH L12. Files: `Cargo.toml:17-18`, `src/plugins/parser.rs`, `src/upgrade/version.rs`.

- [x] **37. Daemon concurrency limit not enforced for upgrade/snapshot; TOCTOU on the check** *(moot — daemon removed per item 4)*
  Source: BUGS L1. Files: ~~`daemon/`~~ (deleted).

- [x] **38. `OperationHandle::cancel` is `async` but awaits nothing; `is_cancellable` ignores completion** *(moot — daemon removed per item 4)*
  Source: BUGS L2. Files: ~~`daemon/src/cancel.rs`~~ (deleted).

- [ ] **39. `count_zypper_upgraded` counts any line containing the substring "done"**
  Source: BUGS L3. Files: `src/backends/os_package_manager.rs:657-659`.

- [ ] **40. fwupd "updated" count shows 0 for reboot-staged firmware**
  Source: BUGS L4. Files: `src/backends/fwupd.rs:178-186`.

- [x] **41. Daemon idle-tracker doesn't refresh during long-running operations** *(moot — daemon removed per item 4)*
  Source: BUGS L5. Files: ~~`daemon/`~~ (deleted).

- [ ] **42. Privileged-shell sentinel token has weak entropy**
  Source: BUGS L6. Files: `src/runner.rs:63-68` (PID + sub-second nanoseconds only).

- [ ] **43. Silent error swallowing across several UI async paths**
  Source: BUGS L7. Files: `src/ui/window.rs:726-728`, `src/ui/upgrade_page.rs:486-490`, `src/history.rs:59-63`.

- [x] **44. Daemon shutdown race: no handling for new operations arriving during idle-poll window; no SIGTERM re-arm** *(moot — daemon removed per item 4)*
  Source: BUGS L8. Files: ~~`daemon/src/main.rs`~~ (deleted).

- [ ] **45. Configurable battery/metered gates**
  Source: FEATURES 12. Files: `src/battery.rs` (hardcoded `capacity < 40`), depends on item 7 (config) landing first.

- [ ] **46. Auto-recheck when VexOS binary cache is syncing**
  Source: FEATURES 13. Files: `UpdateResult::CacheMiss` handling in `window.rs`.

- [ ] **47. Update README feature matrix (fwupd, plugins, Homebrew cleanup, VexOS; fix stale `upgrade.rs` reference)**
  Source: FEATURES 16. Files: `README.md`, `data/io.github.up.metainfo.xml`
  (2.x release note still advertises "pre-update snapshots" — removed in item 18).

---

## Notes

- Items 4, 5, 9, 14, 18, 37, 41, 44 are interdependent around the daemon
  decision — resolving item 4 first avoids rework.
- Items 6, 7, 8, 18, 19, 20 all follow the same pattern (fully-built dead
  module + missing ~20-100 lines of UI glue) — cheapest wins once triaged.
- BUGS.md's "Notes on things that are NOT bugs (verified)" section confirms
  shell-injection guarding, pipe draining, the check-epoch guard, and ANSI
  stripping are all sound — no action needed there.
