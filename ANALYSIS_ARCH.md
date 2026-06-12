# Up — Architecture & Structure Analysis

Date: 2026-06-11
Scope: architecture and structure only (no functional bug hunt, no style nits).
Codebase analyzed: `src/` (main GTK app, ~8,200 lines), `daemon/` (~1,050 lines),
`data/`, `meson.build`, `po/`, CI workflows. Every finding below was verified by
reading the code, not inferred from docs.

Legend: **HIGH** = misleading architecture, broken integration, or dead privileged
surface; **MEDIUM** = significant inconsistency or abandoned subsystem; **LOW** =
local duplication/naming/structure issues.

---

## 1. Architectural anti-patterns / design problems

### H1. The entire D-Bus daemon privilege architecture is built, installed, and never used — HIGH

**Files:**
- `daemon/` (whole crate: `daemon/src/main.rs`, `interface.rs`, `executor.rs`, `allowlist.rs`, `auth.rs`, `audit.rs`, `cancel.rs`, `lifecycle.rs` — ~1,050 lines)
- `src/dbus_client.rs` (entire file, 221 lines)
- `src/main.rs:1-17` (module list)
- `meson.build` (installs `up-daemon` to libexec, `data/io.github.up.Daemon.conf` to `dbus-1/system.d`, `data/io.github.up.Daemon.service` to systemd system units)

**Evidence:** `src/main.rs` does not contain `mod dbus_client;` — the file
`src/dbus_client.rs` is not part of the module tree and **is not even compiled**.
Nothing in `src/` references `DaemonExecutor`, `ExecutionMode`, or
`detect_execution_mode` (all also wrapped in `#[allow(dead_code)]`). The GUI's
only privilege path is `pkexec` via `PrivilegedShell` (`src/runner.rs:34-231`)
and per-command `pkexec` (`src/runner.rs:299-307`).

**Why it's a problem:**
1. **Two parallel privilege architectures exist; the documented one is the dead
   one.** `CLAUDE.md` ("privileged operations delegated to a D-Bus daemon
   (`up-daemon`) via zbus") and `src/plugins/backend.rs:12-14` ("may route
   through the D-Bus daemon") describe an architecture that does not run.
   Anyone onboarding will follow the wrong mental model.
2. **A root daemon is installed as live attack surface with no consumer.**
   Meson installs a root systemd service + a system D-Bus policy that allows
   *any user* to call `io.github.up.Daemon1` methods
   (`data/io.github.up.Daemon.conf`). The daemon executes allowlisted package
   commands as root after a polkit check — code that nothing exercises in CI
   or real usage, yet ships enabled-by-D-Bus-activation on every install.
3. The daemon and the GUI each maintain their own command tables that have
   already diverged (see M5).

**Recommendation:** either wire the client in (`detect_execution_mode()` at
orchestrator construction) or remove the daemon, its data files, and
`src/dbus_client.rs` until the migration is actually scheduled.

### H2. `up --check` background-check integration is broken end-to-end — HIGH

**Files:**
- `data/io.github.up-check.service.in` (`ExecStart=@BINDIR@/up --check`)
- `data/io.github.up-check.timer` (installed `OnCalendar=daily`)
- `src/main.rs:23-28` (no argument parsing at all)
- `src/check.rs:1` (`#![allow(dead_code)]`), `src/check.rs:14` (`run_check`, zero callers)

**Why it's a problem:** Meson installs a daily systemd user timer whose service
runs `up --check`. `main()` registers resources and starts the GTK application
unconditionally — it never looks at `std::env::args()`. The installed timer will
therefore attempt to launch the full GUI daily (and fail in a session without a
usable display, or worse, pop a window). `src/check.rs` (126 lines: stamp file,
notify-send, count aggregation) is a complete implementation of the feature with
its module-level dead-code suppression hiding the fact that nothing calls it.
Note `check.rs:15` also calls `env_logger::init()` a second time, which would
error if it were ever wired in after `main.rs:25`.

### H3. Plugin backends that declare `needs_root: true` cannot actually run privileged — HIGH

**Files:**
- `src/plugins/backend.rs:53-75` (`run_update` executes `runner.run(&cmd.program, …)` directly)
- `src/runner.rs:299-307` (privileged shell is used **only** when `program == "pkexec"`)
- `src/orchestrator.rs:96-117` (auth prompt is raised because `needs_root()` is true)
- `data/backends.d/apk.yaml`, `data/backends.d/xbps.yaml` (`needs_root: true`, `polkit_action: io.github.up.update.system`)

**Why it's a problem:** for the shipped apk/xbps descriptors the orchestrator
opens a `pkexec` shell (user authenticates), but `PluginBackend::run_update`
then invokes `apk upgrade` / `xbps-install -Syu` **directly as the unprivileged
user**, because `CommandRunner` only routes commands through the elevated shell
when the program string is literally `"pkexec"`. The user gets an auth prompt,
then the update fails with a permissions error. The `polkit_action` field in the
descriptor schema is consumed by nothing in the app (it is only meaningful to
the unwired daemon, see H1). The plugin system's privileged path is
half-implemented: the schema, validation (`src/plugins/validate.rs:59-78`), and
auth prompt exist, but the execution step was never connected.

### H4. Per-item selective updates: full backend + orchestrator machinery, no UI — HIGH

**Files:**
- `src/backends/mod.rs:196-223` (`supports_item_selection`, `run_selected_update`; the doc comment at 196-198 describes "per-item checkboxes in the UI" that do not exist)
- Implementations + input validation in every backend: `src/backends/flatpak.rs:205-249`, `src/backends/homebrew.rs:86-123`, `src/backends/os_package_manager.rs:134-176` (APT), `:296-329` (DNF), `:594-627` (Zypper), `src/backends/nix.rs:701-753`
- `src/orchestrator.rs:66` (`BackendSelection = (Arc<dyn Backend>, Option<Vec<String>>)`), `:157-162` (dispatch)
- `src/ui/window.rs:409` (`.map(|b| (b, None))` — the only construction site always passes `None`)
- `src/ui/update_row.rs:124-154` (`set_packages` renders plain rows, no checkboxes, no selection state)

**Why it's a problem:** several hundred lines of selection plumbing, including
five per-backend shell-safety validators, are unreachable. The `Option<Vec<String>>`
in `BackendSelection` complicates the orchestrator's core type for a feature
the UI cannot trigger. The trait documentation actively lies about the UI
("per-item checkboxes … rendered read-only"). Either ship the row checkboxes or
delete the selection path; today it is pure maintenance load that silently rots
(the five validators have already drifted from each other — see L3).

### M1. VexOS vendor coupling baked into the generic Nix backend — MEDIUM

**Files:**
- `src/backends/nix.rs:54-63` (`is_vexos`), `:96-115` (`resolve_nixos_flake_attr`), `:468-490` (`vexos-update` wrapper), `:618-626`
- `src/backends/mod.rs:118-121` (`UpdateResult::CacheMiss` — "exit 2 on VexOS" leaked into the core result enum)

**Why it's a problem:** `resolve_nixos_flake_attr()` makes
`/etc/nixos/vexos-variant` the **only** way to resolve the flake attribute —
on a standard flake-based NixOS machine (no VexOS), `run_update` and
`run_selected_update` fail with an error instructing the user to create a
VexOS-specific file (`nix.rs:109-114`). A vendor-specific convention became a
hard dependency of the generic path (no fallback to hostname or
`nixosConfigurations` enumeration, although the comment at `nix.rs:465-467`
acknowledges the hostname approach). Likewise `CacheMiss` is a distro-specific
semantic hardcoded into the shared `UpdateResult` type and only produced inside
the VexOS branch (`nix.rs:488`).

### M2. Read-only operations bypass the `CommandExecutor` abstraction — MEDIUM

**Files (direct `tokio::process::Command` / `std::process::Command` calls inside backends):**
- `src/backends/flatpak.rs:157-163, 260-264`
- `src/backends/fwupd.rs:47-51, 100-104`
- `src/backends/homebrew.rs:50-54`
- `src/backends/os_package_manager.rs:75-79, 87-94, 244-248, 260-266, 406-410, 426-430, 516-520, 528-534, 550-556`
- `src/backends/nix.rs:179-196, 265-281, 348-358, 384-394, 632-636, 658-662` plus blocking `std::process::Command` in `is_nixos`/`is_vexos`/`is_determinate_nix` (`nix.rs:21-25, 41-45, 56-60, 318-327`)
- `src/plugins/backend.rs:88-93, 106-111`

**Why it's a problem:** `src/executor.rs` defines `CommandExecutor` explicitly
"enabling dependency injection and test doubles", and `run_update` uses it —
but `list_available`, `estimate_size`, and all detection probes spawn processes
directly. Consequences: (a) those paths are untestable with `MockExecutor` —
admitted in the codebase itself at `src/backends/nix.rs:884-892` ("impossible to
exercise in unit tests without a SystemProber abstraction … deferred"); (b) their
output is invisible in the log panel because it never flows through the
`BackendEvent` channel; (c) the modern `nix profile upgrade` branch of
`run_update` (`nix.rs:602`, via `nix_profile_upgrade_all` at `:347-407`) bypasses
the runner *during an update*, so that update produces no streamed log output at
all — inconsistent with every other backend.

### M3. Two unrelated execution stacks for privileged work inside the same app — MEDIUM

**Files:**
- Update path: async `PrivilegedShell` + `CommandRunner` (`src/runner.rs:34-420`), one polkit prompt per session, streamed via `BackendEvent`
- Upgrade path: synchronous `run_command_sync` (`src/runner.rs:434-507`) called from `std::thread::spawn` in `src/upgrade/execute.rs:76-86, 105-119, 137-149, 244-260, 267-275, 282-296, 313-317` — a separate `pkexec` process per command

**Why it's a problem:** the upgrade workflow re-implements process spawning,
pipe draining, and log forwarding (`runner.rs:434-507` duplicates the logic of
`CommandRunner::run` at `:293-392` in blocking form) and pays a polkit prompt
per step: the legacy-NixOS upgrade runs `pkexec` twice (`execute.rs:244` and
`:267`), the Fedora upgrade up to four times (`execute.rs:105`, `:114`, `:137`,
`:165`). The update path solved exactly this problem with `PrivilegedShell`
("The user authenticates exactly once", `runner.rs:31-33`) but the upgrade
subsystem was written against a different, older model and never converged.

### M4. Stringly-typed contracts between layers — MEDIUM

**Files / instances:**
- `src/upgrade/version.rs:21-29` — `check_upgrade_available` returns a human-readable `String`; the UI then decides upgrade availability with `result_msg.starts_with("Yes")` (`src/ui/upgrade_page.rs:482`). Any wording change (or translating these strings, which other code does via gettext) silently flips the "Start Upgrade" gating logic.
- `src/backends/mod.rs:42-69` — `BackendError::from_string` reverse-parses error *prose* ("exited with code", "no such file or directory") to reconstruct typed variants. The comment at `:44` calls it "a bridge during migration from String-based errors", but the migration stalled: `PrivilegedShell::run_command` (`src/runner.rs:116-121`) and `PrivilegedShell::new` (`:50`) still return `Result<_, String>`, so errors are flattened to text and re-parsed at `runner.rs:305`.
- `src/history.rs:9-15` + `src/ui/history_page.rs:105-115, 123-127` — results are matched as raw strings `"success" | "success_self_update" | "error" | "skipped"` with no enum (moot today because nothing writes history, see M6, but it is the persisted schema).

### M5. The daemon allowlist is a second, divergent source of truth for backend commands — MEDIUM

**Files:** `daemon/src/allowlist.rs:34-162` vs the GUI backends.

**Divergences (verified):**
- Nix: daemon runs `nixos-rebuild switch --upgrade` only (`allowlist.rs:128-135`) — no flake support, no `nix flake update`, no Determinate Nix; the GUI (`src/backends/nix.rs:456-611`) handles flakes, channels, VexOS, and Determinate.
- Pacman cleanup: daemon `pacman -Sc --noconfirm` (cache clean, `allowlist.rs:93-100`); GUI removes orphans via `-Qtdq`/`-Rns` (`src/backends/os_package_manager.rs:420-461`). Different operations under the same name.
- `upgrade_commands` is **never populated** (`allowlist.rs:14, 24` — only inserts are for update/cleanup/snapshot), so the daemon's `RunUpgrade` method (`daemon/src/interface.rs:215-220`) returns `InvalidArgs` for every input. A whole D-Bus method that cannot succeed.
- No flatpak/homebrew/fwupd entries; snapshot commands (`allowlist.rs:138-161`) differ in arguments from `src/snapshot.rs:55-118` (e.g. snapper without `-c root -t pre --print-number`).

**Why it's a problem:** if the daemon is ever wired in (H1), behavior changes
silently per backend. If it is not, this is dead config drifting further each
release. Command definitions for "what does updating backend X mean" must live
in one place.

---

## 2. Structural inconsistencies (naming, organization, module boundaries)

### M6. Whole-module `#![allow(dead_code)]` hides seven abandoned subsystems — MEDIUM (inventory; individual items below)

31 `allow(dead_code)` markers across the workspace; 7 are **module-wide**:
`src/check.rs`, `src/config.rs`, `src/history.rs`, `src/ui/history_page.rs`,
`src/snapshot.rs`, `src/changelog.rs`, `src/disk.rs`. Module-level suppression
means the compiler can never tell you when these stop being "temporarily
unwired" and become garbage. See §4 for the per-feature breakdown.

### L1. Duplicate spawn helpers with identical bodies and identically stale docs — LOW

**Files:** `src/orchestrator.rs:197-205` (`spawn_background`) and
`src/ui/mod.rs:10-22` (`spawn_background_async`). Both are
`drop(crate::runtime::runtime().spawn(f()))`. Both doc comments claim "Spawns a
background OS thread" and (ui/mod.rs) "creates a single-threaded Tokio runtime
on that thread" — neither is true since `src/runtime.rs` introduced the shared
multi-thread runtime. Two names for one function, documented as an architecture
that no longer exists.

### L2. Same module names for unrelated concepts across the tree — LOW

- `src/executor.rs` (a trait for DI) vs `daemon/src/executor.rs` (process-spawning implementation) — same name, opposite roles.
- `src/check.rs` (background update count) vs `src/upgrade/check.rs` (upgrade prerequisites) — `crate::check` vs `crate::upgrade::check` invite mis-imports; window.rs already has local closures called `run_checks` referring to a third thing (availability checks).

### L3. Five copies of package-name shell-safety validation, each with different rules — LOW

**Files:**
- APT: allows `+` and `:` (`src/backends/os_package_manager.rs:144-161`)
- DNF: alphanumeric `- . _` only (`:307-318`)
- Zypper: same as DNF but a separate copy (`:605-616`)
- Homebrew: additionally allows `/` (`src/backends/homebrew.rs:97-108`)
- Shared helper `is_safe_pkg_name` (`os_package_manager.rs:702-707`) exists but is used **only** by Zypper cleanup (`:573`)
- Plus `validate_flake_attr` (`src/backends/nix.rs:67-84`) and its acknowledged duplicate `validate_hostname` (`src/upgrade/version.rs:279-296`, itself `#[allow(dead_code)]` — the comment at `:275-277` says both paths "must apply this check", but no upgrade path calls it)

**Why it's a problem:** security-relevant input validation duplicated inline per
backend guarantees drift (it has already drifted). One `validate_pkg_token(charset)`
helper would make the policy auditable in one place.

### L4. i18n applied to half the UI; POTFILES lists files that contain no translatable calls — LOW (MEDIUM if translations are a goal)

**Files:**
- Using gettext: `src/ui/upgrade_page.rs`, `src/ui/history_page.rs`, `src/ui/reboot_dialog.rs`
- Hardcoded English UI strings: `src/ui/window.rs` (e.g. `:41 "Update"`, `:104-107` refresh tooltip, `:186-189`, `:262 "Update All"`, `:270 "Cancel"`, `:325-330` metered dialog, `:352-359` battery dialog, `:549-551` status strings), `src/ui/update_row.rs` (all status labels, `:75-90, 161-227`), `src/ui/log_panel.rs:47, 56, 94-95`
- `po/POTFILES.in` lists `src/ui/window.rs`, `src/ui/update_row.rs`, `src/ui/log_panel.rs` anyway
- **No gettext initialization anywhere:** no `setlocale` / `bindtextdomain` / `textdomain` call exists in `src/` (verified by grep), so even the wrapped strings can never resolve translations. `meson.build` passes `LOCALEDIR` into the cargo environment at build time but nothing in the code reads it.

**Why it's a problem:** the translation infrastructure (po/, meson i18n merge,
gettext-rs dependency) is fully present and fully non-functional. Either
initialize gettext in `main()` and wrap the remaining UI strings, or drop the
machinery.

### L5. `detect_backends()` hardcodes plugin/builtin aliasing as a match table — LOW

**File:** `src/backends/mod.rs:266-281`. The builtin-duplicate check enumerates
every `(BackendKind, &str)` pair by hand. Adding a builtin backend requires
remembering to extend this table or a plugin can shadow/duplicate it. A
`fn plugin_id(&self) -> &str` on `Backend` (or `BackendKind::canonical_id()`)
would remove the parallel list.

### L6. Generated artifact committed beside its source — LOW

**Files:** `data/io.github.up.desktop` (generated) and
`data/io.github.up.desktop.in` (source); meson regenerates the former
(`meson.build`, `i18n.merge_file`). CLAUDE.md documents the trap ("edit the .in
source, not the generated file"), and `preflight.sh` validates the committed
copy — but committed build outputs inevitably go stale relative to the `.in`
and the po files. Prefer validating the meson-built artifact, or generate the
committed copy in CI.

---

## 3. Inconsistent patterns

### M7. The orchestrator event loop is implemented twice in the UI with behavioral drift — MEDIUM

**Files:** `src/ui/window.rs:442-568` ("Update All" handler) vs
`src/ui/window.rs:791-911` (per-row retry closure).

Both consume `OrchestratorEvent` with a near-identical `match`, but:
- The retry path **discards the `CancelHandle`** (`window.rs:815`:
  `orchestrator.run_all(event_tx);`) — a retried backend cannot be cancelled,
  and the Cancel button isn't shown.
- The retry path ignores `SuccessWithSelfUpdate`'s banner side-effect
  (`window.rs:858-864` updates the row only; the main path sets
  `self_updated`/banner at `:505-510, 545-547`).
- The retry path never touches the progress bar or `status_label`.
- Both copies update the same row-status mapping (six `UpdateResult` arms),
  so every new `UpdateResult` variant must be added twice (the `CacheMiss` arm
  already appears in both, `:521-525` and `:878-882`).

This is also the symptom of a structural problem: `build_update_page()` is a
~765-line function (`window.rs:166-930`) returning a 5-tuple
(`UpdatePageResult`, `window.rs:13-19`) — state, orchestration, and widget
construction all live in one closure web. Extracting an event-applier
(`fn apply_event(rows, event, …)`) would eliminate the fork.

### M8. `upgrade_supported` and `execute_upgrade` disagree about supported distros — MEDIUM

**Files:** `src/upgrade/detect.rs:67-77` vs `src/upgrade/execute.rs:19-32`.

`detect_distro()` marks `linuxmint`, `pop`, `elementary`, `zorin`, `debian`,
`rhel`, `centos`, and anything `ID_LIKE`-ubuntu/debian as `upgrade_supported =
true` — which makes the Upgrade tab visible and runs availability checks. But
`execute_upgrade()` matches only `ubuntu | fedora | opensuse-leap | nixos` and
returns "Upgrade is not yet supported for …" for everything else. A Mint/Debian
user can pass all prerequisite checks, tick the backup box, press the
destructive-styled "Start Upgrade", and only then learn it was never
implemented. The two lists must be derived from one table. Relatedly,
`check_upgrade_available` (`src/upgrade/version.rs:22-27`) supports a third,
different set (no debian/rhel handling → "Not supported for this distribution"
subtitle while the tab itself is shown).

### L7. Prerequisite check duplicates backend logic with weaker parsing — LOW

**File:** `src/upgrade/check.rs:44-103`. `check_packages_up_to_date` re-spawns
`apt list --upgradable` / `dnf check-update` / `zypper list-updates` with its
own ad-hoc line counting (`:73-76` filters only empty lines and "Listing…"),
instead of reusing the already-tested `Backend::count_available()` /
`parse_*` functions in `src/backends/`. For DNF the metadata header line
("Last metadata expiration check…") is counted as a pending package, so the
check can fail on a fully-updated Fedora box — a direct consequence of the
duplication (the backend parser at `os_package_manager.rs:332-344` filters
that header; this copy does not).

### L8. Mixed `pkexec` invocation styles across backends — LOW

- APT/Zypper: one `pkexec sh -c "<a> && <b>"` string (`src/backends/os_package_manager.rs:48-57, 489-499`)
- DNF/Pacman: direct argv `pkexec dnf upgrade -y` (`:228, 388`)
- Zypper cleanup builds a shell string by joining validated names (`:581-583`), Pacman cleanup extends argv (`:451-453`)
- Nix wraps everything in `pkexec env PATH=… sh -c …` (`src/backends/nix.rs:470-481, 503-521`); `upgrade/execute.rs:244-253` instead uses `pkexec /usr/bin/env PATH=… nix-channel …` (no shell)

Each style has different quoting/injection characteristics, so the (already
five-way duplicated, L3) validation requirements differ per call site. Pick one
convention (argv-only where possible; one helper for the "&&" case).

### L9. Inconsistent fallback channel naming/format conventions — LOW

Channel payloads in the codebase variously use raw `String` lines
(`run_command_sync`, upgrade pages), `BackendEvent::LogLine` (update path), and
`CheckMsg::{Log,Results}` (`src/ui/upgrade_page.rs:10-15`). The stderr markers
differ too: `run_command_sync` prefixes `"stderr: "` (`src/runner.rs:468`),
the Fedora-upgrade fork prefixes `"[stderr] "` (`src/upgrade/execute.rs:192`),
and `CommandRunner` doesn't mark stderr at all (`runner.rs:350-372`). Cosmetic
individually, but it makes log post-processing (and the `is_nixos_activation_success`
style of output sniffing) fragile.

### L10. Daemon: operation-cleanup poll loop copy-pasted four times — LOW

**File:** `daemon/src/interface.rs:96-113, 171-188, 239-256, 304-321` — the
identical "sleep 100ms, then poll every 1s until `join_handle.is_finished()`"
block appears in `run_update`, `run_cleanup`, `run_upgrade`, and
`create_snapshot`. Besides the duplication, polling a `JoinHandle` is the wrong
primitive — awaiting the handle in a single spawned reaper (or having
`spawn_operation` remove itself from the map on completion) removes both the
duplication and the 1-second latency. Also `idle_timeout_secs` is hardcoded
as `60` in two unconnected places (`daemon/src/main.rs:21-23` and
`interface.rs:408-411`).

---

## 4. Half-implemented or abandoned code

All verified as having **zero callers** outside their own module/tests.

### H5. Update History: page built, never mounted; entries never written — HIGH

**Files:** `src/history.rs` (81 lines; `append_entry` has **no callers** anywhere),
`src/ui/history_page.rs` (148 lines; `HistoryPage::build()` never called —
`src/ui/window.rs:32-52` adds only "update" and "upgrade" pages to the
ViewStack). Both files are `#![allow(dead_code)]`. The page is even listed in
`po/POTFILES.in`. The feature is complete on both ends (storage + UI) and
disconnected in the middle; reconnecting it is ~10 lines (add the stack page;
call `append_entry` in the `BackendFinished` arm), deleting it is ~230.

### M9. Snapshot subsystem: three dead implementations of the same feature — MEDIUM

**Files:**
- `src/snapshot.rs` (119 lines, `#![allow(dead_code)]`): tool detection + `pkexec` creation. Zero callers.
- `src/config.rs` (69 lines, `#![allow(dead_code)]`): `SnapshotPreference` (Ask/Always/Never) and `skipped_backends` persistence. Zero callers — the skip checkboxes in `update_row.rs` are session-only and never saved, despite a config type purpose-built for it.
- `daemon/src/allowlist.rs:138-161` + `daemon/src/interface.rs:262-324`: a third snapshot path (D-Bus `CreateSnapshot`) with different snapper/timeshift arguments, also unreachable (H1).
- `data/io.github.up.policy` ships the `io.github.up.snapshot.create` action for it.

### M10. Disk-space estimation: a complete vertical slice with no consumer — MEDIUM

**Files:** `Backend::estimate_size` default + `#[allow(dead_code)]`
(`src/backends/mod.rs:160-171`), overridden in `flatpak.rs:145-176`,
`fwupd.rs:98-121`, `os_package_manager.rs:85-101, 258-273, 526-538`,
`plugins/backend.rs:101-116` (+ the `estimate_size` command schema in every
YAML descriptor and `apply_parser_size` in `src/plugins/parser.rs:81-120`),
all funneling into `src/disk.rs` (358 lines of parsers, `#![allow(dead_code)]`).
No UI element or orchestrator path calls `estimate_size()` or any `disk::*`
function except from the (equally dead) backends' own methods. The doc comment
at `backends/mod.rs:166-167` claims "This is called alongside `list_available()`
on the background thread" — it is not.

### M11. Cleanup/maintenance: orchestrator + every backend implement it; UI has no button — MEDIUM

**Files:** `CleanupOrchestrator` (`src/orchestrator.rs:207-274`,
`#[allow(dead_code)]`), `supports_cleanup`/`run_cleanup` implemented in
flatpak (`flatpak.rs:178-203`), homebrew (`homebrew.rs:60-84`), nix
(`nix.rs:678-699`), apt/dnf/pacman/zypper (`os_package_manager.rs:103-132,
275-294, 416-461, 540-592`), plugins (`plugins/backend.rs:118-144`), the
`cleanup` command in both shipped YAML descriptors, the daemon's `RunCleanup`
method, and the `io.github.up.cleanup.system` polkit action. Nothing in
`src/ui/` references any of it. This is the largest single block of dead
feature code (~350 lines + config surface) and it spans four layers.

### M12. Changelog fetching — MEDIUM

**File:** `src/changelog.rs` (249 lines, `#![allow(dead_code)]`): per-backend
changelog retrieval (apt/dnf/pacman/zypper/flatpak/brew/fwupd) with timeout
handling. Zero callers. Declared in `main.rs:4` so it compiles, warning-suppressed
so it never surfaces.

### L11. Smaller vestiges — LOW

- `src/backends/flatpak.rs:100` — `let github_self_updated = false;` kept only to feed the `if updated_self || github_self_updated` at `:102`; the SECURITY comment explains the removal, but the variable is dead logic.
- `src/orchestrator.rs:12, 18` — `CancelHandle` itself carries `#[allow(dead_code)]` although it *is* used (`window.rs:280-292`); stale suppression masking real usage information.
- `src/plugins/validate.rs:97-101` — "Version compatibility (basic semver check)" checks only that `min_up_version` is non-empty; no comparison against `CARGO_PKG_VERSION` exists, so the field in every descriptor is decorative.
- `daemon/src/allowlist.rs:166-181` — `register_plugin` (`#[allow(dead_code)]`): the documented frontend→daemon plugin registration ("called by the frontend") was never built; with H1 it cannot be.
- `src/upgrade/version.rs:279-296` — `validate_hostname`, dead duplicate of `validate_flake_attr` (see L3).
- `src/upgrade/detect.rs:5-26` — `DistroInfo`, `NixOsConfigType`, `CheckResult` (`upgrade/check.rs:6-10`) derive `Serialize/Deserialize`; nothing serializes them.
- `data/io.github.up.policy:` legacy actions `io.github.up.pkexec.update` / `.pkexec.upgrade` annotate `/bin/sh` and `/usr/bin/env` exec paths "for backward compatibility during the transition period (v1.x → v2.x)" — the transition (H1) never happened, so the "legacy" path is in fact the only path, and the comment inverts reality.

---

## 5. Dependency findings

### H6. `zbus`, `futures-util`, and `tokio-util` are dead weight in the main crate — HIGH (trivial fix)

**File:** `Cargo.toml:30-32`.
- `zbus = "5"` and `futures-util = "0.3"` are used **only** by
  `src/dbus_client.rs`, which is not in the module tree (H1) and is therefore
  not compiled. They still build (zbus pulls a sizable transitive graph —
  zvariant, proc-macros, etc.) on every CI run and end up in the binary's
  dependency audit surface for nothing.
- `tokio-util = { version = "0.7", features = ["rt"] }` is used by **no file in
  `src/` at all** (only `daemon/src/{cancel,executor}.rs` use it, and the daemon
  declares its own copy in `daemon/Cargo.toml:17`).

Until/unless the daemon client is wired in, all three should be removed from
the root manifest.

### M13. `serde_yml 0.0.12` is a risky choice for the security-sensitive plugin parser — MEDIUM

**Files:** `Cargo.toml:23`, used in `src/plugins/discovery.rs:89`.
`serde_yml` is the contested fork of the archived `serde_yaml`; its 0.0.x
releases (and the underlying `libyml`) have known quality/soundness concerns
and minimal maintenance, and `cargo audit` advisories have been filed against
the project's practices. This crate parses **untrusted-ish input** (YAML from
XDG data dirs, explicitly size-capped and validated for security in
`discovery.rs:9-10` and `validate.rs`). Given the threat model the project
itself documents, a maintained parser (`serde_yaml_ng`, or going through
`saphyr`) — or pinning + auditing the current one — deserves a deliberate
decision rather than an incidental pick.

### L12. Minor dependency notes — LOW

- `glib = "0.20"` / `gio = "0.20"` (`Cargo.toml:17-18`) are declared separately
  although `gtk4` re-exports compatible versions (`gtk::glib`, `gtk::gio` are
  already used in `src/ui/window.rs:7-8`). Two sources for the same crates can
  drift on a bump; importing through the gtk re-exports is the common gtk-rs
  convention.
- `regex = "1"` is used only by `src/plugins/parser.rs`, where every call
  re-compiles the pattern per invocation (`parser.rs:12, 17, 65, 87` —
  `Regex::new` inside per-line loops' enclosing fn). Fine at current scale, but
  the parser is invoked per backend per check; caching compiled patterns in the
  descriptor would also surface invalid patterns at validation time instead of
  silently returning 0/empty (`parser.rs:14, 19-23, 71` swallow `Err(_)`).
- `ureq` (blocking HTTP) is used only in `src/upgrade/version.rs` from spawned
  threads — consistent with the upgrade path's sync style (M3), but it is the
  only blocking-HTTP island in an otherwise Tokio app; if M3 is ever unified,
  this becomes `reqwest`-or-spawn_blocking territory.

---

## Summary table

| # | Priority | Finding | Primary location |
|---|----------|---------|------------------|
| H1 | High | D-Bus daemon architecture built+installed, never wired; client file not even compiled | `daemon/`, `src/dbus_client.rs`, `src/main.rs:1-17` |
| H2 | High | Installed daily timer runs `up --check`; binary has no arg handling; `check.rs` dead | `data/io.github.up-check.service.in`, `src/main.rs:23-28`, `src/check.rs` |
| H3 | High | `needs_root` plugin backends auth via pkexec shell but execute unprivileged | `src/plugins/backend.rs:53-75`, `src/runner.rs:299-307` |
| H4 | High | Selective-update machinery in all backends + orchestrator; UI always passes `None`, no checkboxes | `src/ui/window.rs:409`, `src/backends/mod.rs:196-223` |
| H5 | High | History feature: storage + page complete, never mounted, never written | `src/history.rs`, `src/ui/history_page.rs`, `src/ui/window.rs:32-52` |
| H6 | High | `zbus`/`futures-util`/`tokio-util` unused in root crate | `Cargo.toml:30-32` |
| M1 | Medium | VexOS vendor coupling hard-wired into generic Nix backend and core `UpdateResult` | `src/backends/nix.rs:96-115`, `src/backends/mod.rs:118-121` |
| M2 | Medium | Read-only ops bypass `CommandExecutor` (untestable, unlogged) | backends, `src/plugins/backend.rs:88-111` |
| M3 | Medium | Parallel sync/async privileged-exec stacks; upgrade path prompts per command | `src/runner.rs:434-507`, `src/upgrade/execute.rs` |
| M4 | Medium | Stringly-typed contracts: `starts_with("Yes")`, error-prose re-parsing | `src/ui/upgrade_page.rs:482`, `src/backends/mod.rs:42-69` |
| M5 | Medium | Daemon allowlist diverged from GUI commands; `upgrade_commands` empty → `RunUpgrade` can never succeed | `daemon/src/allowlist.rs` |
| M6 | Medium | 7 modules under blanket `#![allow(dead_code)]` | see §2 |
| M7 | Medium | Orchestrator event loop duplicated in UI; retry path loses cancel + banner | `src/ui/window.rs:442-568` vs `:791-911` |
| M8 | Medium | `upgrade_supported` distro list ≠ `execute_upgrade` distro list | `src/upgrade/detect.rs:67-77` vs `execute.rs:19-32` |
| M9 | Medium | Snapshot feature: three dead implementations (app, config, daemon) | `src/snapshot.rs`, `src/config.rs`, daemon |
| M10 | Medium | `estimate_size` + `disk.rs` slice: implemented everywhere, consumed nowhere | `src/disk.rs`, all backends |
| M11 | Medium | Cleanup feature implemented across 4 layers, no UI entry point | `src/orchestrator.rs:207-274` et al. |
| M12 | Medium | `changelog.rs` fully dead | `src/changelog.rs` |
| M13 | Medium | `serde_yml 0.0.12` parses semi-trusted plugin YAML | `Cargo.toml:23` |
| L1 | Low | Duplicate spawn helpers with stale docs | `src/orchestrator.rs:197-205`, `src/ui/mod.rs:10-22` |
| L2 | Low | Colliding module names (`executor`, `check`) across crates/trees | — |
| L3 | Low | 5 divergent inline package-name validators + 2 flake-attr validators | backends |
| L4 | Low | i18n half-applied; gettext never initialized; POTFILES stale | `src/ui/*`, `po/POTFILES.in` |
| L5 | Low | Hardcoded plugin/builtin alias table | `src/backends/mod.rs:266-281` |
| L6 | Low | Generated `.desktop` committed beside `.in` | `data/` |
| L7 | Low | Upgrade prereq check re-implements backend parsing, weaker | `src/upgrade/check.rs:44-103` |
| L8 | Low | Mixed pkexec invocation styles (`sh -c` vs argv vs `env`) | backends, `src/upgrade/execute.rs` |
| L9 | Low | Three log-channel conventions, three stderr prefixes | `src/runner.rs`, `src/upgrade/execute.rs` |
| L10 | Low | Daemon cleanup poll loop ×4; double-hardcoded idle timeout | `daemon/src/interface.rs` |
| L11 | Low | Misc vestiges (dead flags, decorative `min_up_version`, inverted "legacy" comments) | see §4 |
| L12 | Low | Minor dep notes (`glib`/`gio` duplication, per-call regex compile, `ureq` island) | `Cargo.toml`, `src/plugins/parser.rs` |

## Cross-cutting observation

The repository is mid-flight in a v1→v2 architecture migration that stalled:
v2.0 introduced the daemon, the plugin system, history, snapshots, cleanup,
size estimation, and selective updates (git: `909cb7a feat: add D-Bus backend
service and plugin discovery system (v2.0)`), but only the plugin *discovery*
half landed in the running app. Roughly 1,800–2,000 lines (≈20% of the
workspace) are currently unreachable from `main()`, much of it privileged-
operation code, and the blanket `#![allow(dead_code)]` markers prevent the
compiler from reporting any of it. The single highest-leverage structural
decision is to either finish the daemon migration (which collapses H1, H3, M5,
M9, and the "legacy" polkit actions into one design) or excise it and let the
pkexec architecture be the documented, intentional one.
