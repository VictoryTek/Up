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

- [ ] **1. `AuthFailed` event leaves `updating` flag stuck `true`, wedging the UI**
  Source: BUGS H2.
  Files: `src/ui/window.rs:512-521`.
  The `AuthFailed` arm in the "Update All" event loop `return`s without calling
  `updating.set(false)`. Cancelling/failing the polkit prompt permanently
  disables the Refresh button and all per-row Retry buttons until restart.

- [ ] **2. `up --check` is completely broken — no CLI argument handling in `main()`**
  Source: ARCH H2, BUGS H1, FEATURES 1.
  Files: `src/main.rs:23-28`, `src/check.rs` (whole file, dead), `data/io.github.up-check.service.in`.
  `main()` never inspects `std::env::args()`; the daily systemd timer's
  `up --check` is handed straight to GTK/GApplication, which rejects the
  unknown option and fails every day. `check.rs` is a complete, unused
  implementation (stamp file, notify-send, count aggregation). Note:
  `check::run_check()` also calls `env_logger::init()` a second time — must
  reconcile with `main.rs`'s existing call before wiring in.

- [ ] **3. Plugin backends with `needs_root: true` authenticate but then run unprivileged**
  Source: ARCH H3.
  Files: `src/plugins/backend.rs:53-75`, `src/runner.rs:299-307`, `src/orchestrator.rs:96-117`, `data/backends.d/apk.yaml`, `data/backends.d/xbps.yaml`.
  `CommandRunner` only routes through the elevated shell when the program
  string is literally `"pkexec"`. Plugin backends prompt the user for admin
  auth, then execute the actual command directly as the unprivileged user —
  the update always fails with a permissions error after wasting an auth
  prompt.

- [ ] **4. Decide the fate of the D-Bus daemon: wire it in, or remove it**
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

- [ ] **5. Remove unused dependencies `zbus`, `futures-util`, `tokio-util` from root crate**
  Source: ARCH H6.
  Files: `Cargo.toml:30-32`.
  Only used by `src/dbus_client.rs`, which isn't compiled. Depends on the
  outcome of item 4 — skip/redo if the daemon gets wired in instead.

- [ ] **6. Wire up the Update History page**
  Source: ARCH H5, BUGS M2, FEATURES 2.
  Files: `src/history.rs`, `src/ui/history_page.rs`, `src/ui/window.rs:32-52` (ViewStack), `BackendFinished` handlers at `window.rs:442-568` and `:791-911`.
  Storage layer and UI page are both fully built and dead. Needs: (a) add a
  third ViewStack page, (b) call `history::append_entry()` from the
  `BackendFinished` arms in both the Update-All loop and the retry loop.

- [ ] **7. Persist user preferences (skip-backend choices) across restarts**
  Source: FEATURES 3 (related dead code also noted in ARCH M9, BUGS M4).
  Files: `src/config.rs` (dead, zero callers), `src/ui/update_row.rs` (skip checkboxes, session-only).
  `AppConfig` with JSON load/save already exists. Needs: load config on
  backend-detection completion and pre-set checkboxes; save on
  `on_skip_changed`.

- [ ] **8. Wire up Cleanup / maintenance mode**
  Source: ARCH M11, BUGS M4, FEATURES 4.
  Files: `src/orchestrator.rs:207-274` (`CleanupOrchestrator`, dead), every backend's `run_cleanup()`/`supports_cleanup()`, `src/ui/window.rs` (no entry point).
  Every backend already implements real cleanup logic (`apt autoremove`,
  `nix-collect-garbage -d`, etc.) and a finished orchestrator reuses the
  existing event/auth/log pipeline — there is simply no button. Add a "Clean
  Up" menu entry that drives `CleanupOrchestrator::run_all()`.

- [ ] **9. Make per-package selective updates real (checkboxes in the UI)**
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

- [ ] **10. VexOS vendor coupling hard-wired into the generic Nix backend**
  Source: ARCH M1.
  Files: `src/backends/nix.rs:54-63, 96-115, 468-490, 618-626`, `src/backends/mod.rs:118-121`.
  `/etc/nixos/vexos-variant` is the *only* way to resolve the flake attribute;
  plain flake-based NixOS users get an error telling them to create a
  VexOS-specific file. `UpdateResult::CacheMiss` is also a VexOS-only concept
  baked into the shared result enum.

- [ ] **11. Read-only backend operations bypass the `CommandExecutor` abstraction**
  Source: ARCH M2.
  Files: `src/backends/flatpak.rs`, `fwupd.rs`, `homebrew.rs`, `os_package_manager.rs`, `nix.rs`, `src/plugins/backend.rs` (many direct `Command` call sites listed in ANALYSIS_ARCH.md §1 M2).
  `list_available`, `estimate_size`, and detection probes spawn processes
  directly instead of through `CommandExecutor`, making them untestable with
  `MockExecutor` and invisible in the log panel. The `nix profile upgrade`
  branch also produces zero streamed log output during an actual update.

- [ ] **12. Unify the two privileged-execution stacks (update vs. upgrade)**
  Source: ARCH M3.
  Files: `src/runner.rs:34-420` (async, one auth prompt) vs `src/runner.rs:434-507` + `src/upgrade/execute.rs` (sync, `pkexec` per command — up to 4 prompts for Fedora upgrade).
  The upgrade path re-implements process spawning/pipe draining that
  `PrivilegedShell` already solved.

- [ ] **13. Replace stringly-typed contracts between layers**
  Source: ARCH M4.
  Files: `src/ui/upgrade_page.rs:482` (`result_msg.starts_with("Yes")`), `src/backends/mod.rs:42-69` (`BackendError::from_string` re-parses error prose), `src/runner.rs:50, 116-121`, `src/history.rs:9-15`.
  A wording change or gettext translation of these strings would silently
  break upgrade-availability gating.

- [ ] **14. Daemon allowlist diverged from GUI commands / `RunUpgrade` can never succeed**
  Source: ARCH M5 (also see item 4 — resolve together).
  Files: `daemon/src/allowlist.rs`.
  Nix, pacman-cleanup, and snapshot commands differ from what the GUI
  actually runs; `upgrade_commands` is never populated so `RunUpgrade` always
  returns `InvalidArgs`. Moot if the daemon is removed per item 4.

- [ ] **15. Remove blanket `#![allow(dead_code)]` from the 7 abandoned-subsystem modules**
  Source: ARCH M6.
  Files: `src/check.rs`, `src/config.rs`, `src/history.rs`, `src/ui/history_page.rs`, `src/snapshot.rs`, `src/changelog.rs`, `src/disk.rs`.
  Do this incrementally as each subsystem gets wired up (items 2, 6, 7, 16,
  19) or deliberately deleted — module-wide suppression currently hides real
  dead-code warnings.

- [ ] **16. Orchestrator event loop duplicated in the UI with behavioral drift**
  Source: ARCH M7.
  Files: `src/ui/window.rs:442-568` (Update All) vs `:791-911` (retry).
  The retry path drops the `CancelHandle` (retried backend can't be
  cancelled), ignores the self-update banner, and never touches the progress
  bar. Every new `UpdateResult` variant must currently be handled in both
  places. Consider extracting a shared `apply_event()` function.

- [ ] **17. `upgrade_supported` and `execute_upgrade` disagree about supported distros**
  Source: ARCH M8.
  Files: `src/upgrade/detect.rs:67-77` vs `src/upgrade/execute.rs:19-32` vs `src/upgrade/version.rs:22-27` (a third, different list).
  A Mint/Debian user can pass all prerequisite checks and press "Start
  Upgrade" only to be told it was never implemented. Derive all three lists
  from one table.

- [ ] **18. Wire up or delete the snapshot subsystem (Timeshift/Snapper/btrfs)**
  Source: ARCH M9, BUGS M4, FEATURES 5.
  Files: `src/snapshot.rs` (dead), `src/config.rs::SnapshotPreference` (dead), `daemon/src/allowlist.rs` + `interface.rs` (third, also-dead implementation).
  Detection and `pkexec`-based creation for all three tools already exist.
  Biggest trust feature for an app that runs unattended system upgrades —
  worth prioritizing if picked up. Depends on item 4 if using the daemon path.

- [ ] **19. Wire up disk-size estimation in the update rows**
  Source: ARCH M10, BUGS M3, FEATURES 6.
  Files: `src/backends/mod.rs:160-171` (`estimate_size`, dead), `src/disk.rs` (dead), all backend overrides.
  `estimate_size()` is implemented for every backend but nothing calls it.
  Would enable "12 updates available (~450 MB)" and a low-disk-space warning.

- [ ] **20. `changelog.rs` is fully implemented but has zero callers**
  Source: ARCH M12, BUGS M3, FEATURES 8.
  Files: `src/changelog.rs` (249 lines, dead).
  Per-backend changelog fetchers with timeout handling exist; add a
  "What's new" button/dialog per row to surface it.

- [ ] **21. Replace `serde_yml 0.0.12` with a maintained YAML parser**
  Source: ARCH M13.
  Files: `Cargo.toml:23`, `src/plugins/discovery.rs:89`.
  `serde_yml` is an unmaintained fork with soundness concerns, used to parse
  plugin descriptor YAML the project itself treats as semi-trusted input.
  Consider `serde_yaml_ng` or `saphyr`.

- [ ] **22. Fix package-count miscounting for APT selective updates and DNF/generic prerequisite checks**
  Source: BUGS M5 & M6 (also ARCH L7 — same DNF issue reported twice).
  Files: `src/backends/os_package_manager.rs:138-176, 189-201` (APT `count_apt_upgraded` returns 0 when already-current), `src/upgrade/check.rs:44-103` (`check_packages_up_to_date` counts DNF's metadata-expiration header line as a pending package, can block upgrade on a clean Fedora system).
  Both should reuse the already-tested `parse_*`/`count_available()` logic in
  `src/backends/` instead of ad-hoc line counting.

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

- [ ] **34. Daemon operation-cleanup poll loop copy-pasted four times; idle timeout hardcoded twice**
  Source: ARCH L10. Files: `daemon/src/interface.rs:96-113, 171-188, 239-256, 304-321`; `daemon/src/main.rs:21-23` vs `interface.rs:408-411`.

- [ ] **35. Misc vestiges: dead flags, decorative `min_up_version` check, inverted "legacy" polkit comment**
  Source: ARCH L11. Files: `src/backends/flatpak.rs:100`, `src/orchestrator.rs:12,18`, `src/plugins/validate.rs:97-101`, `daemon/src/allowlist.rs:166-181`, `data/io.github.up.policy`.

- [ ] **36. Minor dependency cleanups: duplicate `glib`/`gio` sourcing, per-call regex recompilation, `ureq` as the lone blocking-HTTP island**
  Source: ARCH L12. Files: `Cargo.toml:17-18`, `src/plugins/parser.rs`, `src/upgrade/version.rs`.

- [ ] **37. Daemon concurrency limit not enforced for upgrade/snapshot; TOCTOU on the check**
  Source: BUGS L1. Files: `daemon/src/interface.rs:194-324`. Moot until item 4 resolves the daemon's fate.

- [ ] **38. `OperationHandle::cancel` is `async` but awaits nothing; `is_cancellable` ignores completion**
  Source: BUGS L2. Files: `daemon/src/cancel.rs:15-26`.

- [ ] **39. `count_zypper_upgraded` counts any line containing the substring "done"**
  Source: BUGS L3. Files: `src/backends/os_package_manager.rs:657-659`.

- [ ] **40. fwupd "updated" count shows 0 for reboot-staged firmware**
  Source: BUGS L4. Files: `src/backends/fwupd.rs:178-186`.

- [ ] **41. Daemon idle-tracker doesn't refresh during long-running operations**
  Source: BUGS L5. Files: `daemon/src/lifecycle.rs:44-57`, `daemon/src/interface.rs:80`. Latent bug, moot until item 4.

- [ ] **42. Privileged-shell sentinel token has weak entropy**
  Source: BUGS L6. Files: `src/runner.rs:63-68` (PID + sub-second nanoseconds only).

- [ ] **43. Silent error swallowing across several UI async paths**
  Source: BUGS L7. Files: `src/ui/window.rs:726-728`, `src/ui/upgrade_page.rs:486-490`, `src/history.rs:59-63`.

- [ ] **44. Daemon shutdown race: no handling for new operations arriving during idle-poll window; no SIGTERM re-arm**
  Source: BUGS L8. Files: `daemon/src/main.rs:41-48`. Related to item 41.

- [ ] **45. Configurable battery/metered gates**
  Source: FEATURES 12. Files: `src/battery.rs` (hardcoded `capacity < 40`), depends on item 7 (config) landing first.

- [ ] **46. Auto-recheck when VexOS binary cache is syncing**
  Source: FEATURES 13. Files: `UpdateResult::CacheMiss` handling in `window.rs`.

- [ ] **47. Update README feature matrix (fwupd, plugins, Homebrew cleanup, VexOS; fix stale `upgrade.rs` reference)**
  Source: FEATURES 16. Files: `README.md`.

---

## Notes

- Items 4, 5, 9, 14, 18, 37, 41, 44 are interdependent around the daemon
  decision — resolving item 4 first avoids rework.
- Items 6, 7, 8, 18, 19, 20 all follow the same pattern (fully-built dead
  module + missing ~20-100 lines of UI glue) — cheapest wins once triaged.
- BUGS.md's "Notes on things that are NOT bugs (verified)" section confirms
  shell-injection guarding, pipe draining, the check-epoch guard, and ANSI
  stripping are all sound — no action needed there.
