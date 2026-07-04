# Up — Bug & Code-Quality Analysis

Analysis of the `Up` GTK4 system updater (Rust) covering logic errors, security,
performance, dead code, and error handling. Findings are grouped by priority.
Line numbers refer to the state of the tree at the time of analysis.

---

## HIGH PRIORITY

### H1 — `up --check` is completely broken: no CLI argument handling in `main()`
**Files:** [src/main.rs:23-28](src/main.rs#L23-L28), [src/check.rs:14](src/check.rs#L14), [data/io.github.up-check.service.in:7](data/io.github.up-check.service.in#L7)

`main()` never inspects `std::env::args`. It unconditionally builds and runs the
GTK application:

```rust
fn main() -> gtk::glib::ExitCode {
    gio::resources_register_include!("compiled.gresource")...;
    env_logger::init();
    let app = UpApplication::new();
    app.run()   // <-- passes argv straight into GApplication
}
```

The packaged systemd unit runs `@BINDIR@/up --check` on a daily timer
(`io.github.up-check.service.in`). Because `adw::Application::run()` hands argv to
GApplication and `--check` is not a registered option, GApplication prints
"Unknown option --check" and exits non-zero **every time the timer fires**. The
entire "daily update available" desktop-notification feature (implemented in
`src/check.rs`) never runs — `run_check()` has zero callers.

**What goes wrong:** The advertised background update-notification feature is
dead; the systemd timer logs a failure daily. A user relying on notifications
silently never receives them.

**Secondary risk:** If someone "fixes" this by calling `check::run_check()` from
`main()`, note that `run_check()` calls `env_logger::init()` (check.rs:15) while
`main()` also calls it (main.rs:26) — the second `init()` panics. Both need
reconciling.

---

### H2 — Auth failure in the main "Update All" flow permanently wedges the UI
**File:** [src/ui/window.rs:512-521](src/ui/window.rs#L512-L521)

In the primary update event loop, the `AuthFailed` arm returns early **without
resetting the `updating` flag**:

```rust
OrchestratorEvent::AuthFailed(e) => {
    log_panel.append_line(&format!("Authentication failed: {e}"));
    status_label.set_label("Update cancelled.");
    progress_bar.set_visible(false);
    *cancel_handle.borrow_mut() = None;
    cancel_button.set_visible(false);
    cancel_button.set_sensitive(true);
    button.set_sensitive(true);
    return;                       // <-- updating.set(false) is NEVER called
}
```

Every other exit path from this loop (the code after the `while` at
window.rs:601) calls `updating.set(false)`. The `updating` cell is the same
`Rc<Cell<bool>>` returned as `update_in_progress` and consulted by:
- the header **Refresh** button (window.rs:120: `if update_in_progress.get() { return; }`)
- every per-row **Retry** closure (window.rs:815, 830: `if updating_*.get() { return; }`)

**What goes wrong:** If the user cancels or fails the polkit prompt during
"Update All" (a common case — hitting Escape on the password dialog), `updating`
stays `true` forever. The Refresh button and all Retry buttons become
permanently inert until the app is restarted. The "Update All" button itself
still works only because its own click handler re-sets `updating` unconditionally.

**Fix:** add `updating.set(false);` in the `AuthFailed` arm before `return`.

---

## MEDIUM PRIORITY

### M1 — The entire privileged D-Bus daemon is built and shipped but never used by the app
**Files:** [src/dbus_client.rs](src/dbus_client.rs) (whole file), [daemon/](daemon/) (whole crate), [src/backends/mod.rs](src/backends/mod.rs), [src/runner.rs:34](src/runner.rs#L34)

`DaemonExecutor`, `detect_execution_mode()`, and `cancel_operation()` in
`dbus_client.rs` are all `#[allow(dead_code)]` and have **no callers**. The GUI
performs every privileged operation through `PrivilegedShell` (`pkexec /bin/sh`)
in `runner.rs`. The `up-daemon` crate — a full polkit-authenticated D-Bus
service with an allowlist, audit log, cancellation, and idle lifecycle — is
compiled, packaged (`data/io.github.up.Daemon.service`), and granted polkit
actions, yet the frontend never connects to it.

**What goes wrong:**
- Large maintenance burden: two parallel privileged-execution implementations,
  only one of which runs.
- Security surface: a privileged system D-Bus service is installed and
  activatable but serves no client — unnecessary attack surface.
- The polkit policy advertises `io.github.up.update.system` etc. (daemon
  actions) while the app actually authenticates against the legacy
  `io.github.up.pkexec.*` actions. Confusing and easy to mis-audit.

**Recommendation:** Either wire the GUI to the daemon (preferred — it's the more
secure design: fixed allowlist vs. a live root shell) or remove the daemon crate
and its packaging/policy entries. Do not ship both.

---

### M2 — Update history is never recorded; History page is dead
**Files:** [src/history.rs:32](src/history.rs#L32), [src/ui/history_page.rs](src/ui/history_page.rs) (whole file)

`history::append_entry()` — the only function that writes a history record — has
**no callers**. `HistoryPage::build()` also has no callers (window.rs adds only
the Update and Upgrade pages to the ViewStack). The `HistoryEntry` struct,
JSONL serialisation, and the Clear button are all present but the feature is
entirely disconnected.

**What goes wrong:** If the History page were ever added to the UI it would
always show "No history yet", because no update path calls `append_entry()`.
The `history_page.rs` file carries `#![allow(dead_code)]` to suppress the
warnings, masking the fact that a shipped-looking feature does nothing.

---

### M3 — Disk-size estimation and changelog features are fully implemented but unreachable
**Files:** [src/backends/mod.rs:169](src/backends/mod.rs#L169), [src/disk.rs](src/disk.rs), [src/changelog.rs](src/changelog.rs)

- Every backend implements `estimate_size()` (APT/DNF/Zypper/Flatpak/fwupd/plugin),
  and `disk.rs` has extensive tested parsers — but **nothing in `src/ui/` or
  `orchestrator.rs` ever calls `estimate_size()`**. `disk::detect_available_space`
  and `disk::format_bytes` also have no callers.
- `changelog::fetch_changelog()` (250 lines, per-backend changelog fetching) has
  **no callers** anywhere.

**What goes wrong:** Substantial code (with unit tests and network/subprocess
logic) is compiled but never exercised in production. `#![allow(dead_code)]` at
the top of `disk.rs` and `changelog.rs` hides this. Users never see size
estimates or changelogs despite the machinery existing.

---

### M4 — `snapshot.rs`, `config.rs`, and `CleanupOrchestrator` are dead
**Files:** [src/snapshot.rs](src/snapshot.rs), [src/config.rs](src/config.rs), [src/orchestrator.rs:209-274](src/orchestrator.rs#L209-L274)

- `snapshot::create_snapshot()` / `detect_snapshot_tool()` — no callers. The
  daemon has a `create_snapshot` D-Bus method and polkit action, but the GUI
  never triggers a snapshot. The `SnapshotPreference` config field
  (`config.rs`) is therefore also never consulted.
- `config::load_config()` / `save_config()` — no callers. User preferences
  (skipped backends, snapshot policy) are never persisted or restored; the
  skip checkboxes reset every launch.
- `CleanupOrchestrator` (orchestrator.rs:209) — no callers. Every backend
  implements `run_cleanup()` but there is no UI path to invoke maintenance.

**What goes wrong:** Three more advertised-looking capabilities (pre-update
snapshots, persisted preferences, cleanup) are inert. The skip-backend UX in
particular is misleading: users can uncheck a backend but the choice is lost on
restart because `save_config` is never called.

---

### M5 — `count_apt_upgraded` returns 0 for `apt-get install --only-upgrade`
**Files:** [src/backends/os_package_manager.rs:138-176](src/backends/os_package_manager.rs#L138-L176), [os_package_manager.rs:189-201](src/backends/os_package_manager.rs#L189-L201)

`run_selected_update` for APT runs `apt-get install --only-upgrade -y <pkgs>`
and counts results with `count_apt_upgraded()`, which scans for a line
containing `"upgraded"` and parses the leading integer. `apt-get install`'s
summary line is `"N upgraded, M newly installed, ..."` so this usually works —
but when apt decides packages are already current it prints
`"0 upgraded, 0 newly installed"` and the count is 0 even though the operation
succeeded, and when apt uses a localised or differently-worded summary the count
silently falls to 0.

**What goes wrong:** The per-row "N updated" figure after a *selected* APT update
is unreliable (frequently shows "Up to date" after a successful partial upgrade).
Low user-facing severity but a correctness defect in the reported count. The same
brittle first-token/`contains("upgraded")` heuristic is shared with the full
update path.

---

### M6 — `check_packages_up_to_date` miscounts DNF/generic output
**File:** [src/upgrade/check.rs:71-95](src/upgrade/check.rs#L71-L95)

The prerequisite "all packages up to date" check counts every non-empty stdout
line that doesn't start with `"Listing"`:

```rust
let upgradable = stdout.lines()
    .filter(|l| !l.is_empty() && !l.starts_with("Listing"))
    .count();
```

For `dnf check-update` the output includes a `"Last metadata expiration
check: ..."` header line and blank-separated sections (Obsoleting, Security).
The header and any section text are counted as "packages", so a Fedora system
that is actually up to date can report ">0 packages need updating first" and
block the upgrade. The APT branch is only correct because
`apt list --upgradable` prints just the `Listing...` header when clean.

**What goes wrong:** False "not up to date" prerequisite failures on Fedora,
gating the distro-upgrade button incorrectly. Should filter on package-shaped
lines (as `parse_dnf_list_upgrades` already does) rather than a raw line count.

---

## LOW PRIORITY

### L1 — Daemon concurrency limit not enforced for upgrade/snapshot
**File:** [daemon/src/interface.rs:194-324](daemon/src/interface.rs#L194-L324)

`run_update` (line 67) and `run_cleanup` (line 147) check
`ops.len() >= MAX_CONCURRENT_OPS`, but `run_upgrade` and `create_snapshot` do
not. Additionally, even where checked, the check-then-insert is a TOCTOU: the
lock is released (`drop(ops)`) before the insert, so two concurrent callers can
both pass the check and exceed the limit. Not security-critical (polkit gates
each call), but the `MAX_CONCURRENT_OPS` guarantee is not actually upheld.
(Currently moot because the daemon has no client — see M1.)

### L2 — `OperationHandle::cancel` is `async` but awaits nothing; `is_cancellable` ignores completion
**File:** [daemon/src/cancel.rs:15-26](daemon/src/cancel.rs#L15-L26)

`cancel()` is declared `async` yet contains no `.await`; it only flips an atomic
token. `is_cancellable()` returns `true` for any operation whose token isn't
cancelled, including one whose task has already finished but hasn't yet been
reaped by the 1s cleanup poller — so `list_operations` can advertise a finished
op as cancellable.

### L3 — `count_zypper_upgraded` counts any line containing "done"
**File:** [src/backends/os_package_manager.rs:657-659](src/backends/os_package_manager.rs#L657-L659)

```rust
output.lines().filter(|l| l.contains("done")).count()
```

Any zypper progress/status line containing the substring "done" (not only
`Retrieving package ...done`) inflates the updated count. Cosmetic (the number
shown to the user), but imprecise.

### L4 — fwupd "updated" count is 0 for reboot-staged firmware
**File:** [src/backends/fwupd.rs:178-186](src/backends/fwupd.rs#L178-L186)

`count_fwupd_updated` counts `"Successfully installed"` / `"Updated "` lines.
Most firmware updates are *staged for the next reboot* and emit neither, so a
successful `fwupdmgr update` reports `0 updated`. The code comment acknowledges
this is "still correct," but the UI shows "Up to date" after genuinely queuing
firmware — misleading. Consider surfacing "staged for reboot" explicitly.

### L5 — Idle-tracker polling holds no coordination with active operations
**Files:** [daemon/src/lifecycle.rs:44-57](daemon/src/lifecycle.rs#L44-L57), [daemon/src/interface.rs:80](daemon/src/interface.rs#L80)

`mark_active()` is called only at operation *start*. A long-running update
(e.g. `nixos-rebuild`, up to the 1-hour command timeout) does not periodically
refresh the idle timer. The 60s idle check only looks at `last_activity`; if an
operation runs longer than 60s with no new D-Bus calls the tracker still reports
non-idle only because... it doesn't — `is_idle()` compares `last_activity`
(set at start) to now, so after 60s of a running op the daemon believes it is
idle and `wait_for_shutdown` returns, potentially tearing down the connection
mid-operation. (Again moot until the daemon has a client — M1 — but a latent bug
if it is ever wired up.) Operations should refresh `mark_active` while running,
or `is_idle` should consult `operations.is_empty()`.

### L6 — `session_id` entropy is weak
**File:** [src/runner.rs:63-68](src/runner.rs#L63-L68)

The privileged-shell sentinel token is `format!("{:x}_{:x}", pid, subsec_nanos)`
— only the sub-second nanosecond component of the current time plus the PID. This
is low-entropy and predictable. It is defence-in-depth against a subprocess
spoofing the exit-code sentinel, so impact is limited, but a random value
(e.g. from `getrandom`/`uuid`) would be strictly better for the stated purpose.

### L7 — Silent error swallowing across UI async paths
**Files:** [src/ui/window.rs:726-728](src/ui/window.rs#L726-L728), [src/ui/upgrade_page.rs:486-490](src/ui/upgrade_page.rs#L486-L490), [src/history.rs:59-63](src/history.rs#L59-L63)

Several places discard errors with no user feedback:
- window.rs:727 `Err(_) => row.set_packages(&[])` — a `list_available` failure is
  rendered identically to "no packages pending"; the user cannot distinguish a
  broken source from an up-to-date one at the package-list level (the count path
  does flag it via `set_status_unknown`, but the list path does not).
- `history::load_entries` (history.rs:61) `filter_map(... .ok())` silently drops
  any corrupt line — acceptable for forward-compat but a wholly corrupt file
  reads as empty history with no warning.

These are intentional-looking `let _ =`/`.ok()` swallows; individually minor but
collectively they make failures invisible.

### L8 — `is_idle` / shutdown race can drop the bus name mid-call (see L5) — plus no SIGTERM re-arm
**File:** [daemon/src/main.rs:41-48](daemon/src/main.rs#L41-L48)

The `tokio::select!` treats idle-timeout and `ctrl_c` as terminal. There is no
handling for the case where a new operation arrives during the 5s idle poll
window; combined with L5 this widens the window for shutting down while work is
pending.

---

## Notes on things that are NOT bugs (verified)

- **Shell-injection guarding is sound.** `runner::shell_quote` uses the
  canonical `'\''` idiom and control-character rejection; backend
  `run_selected_update` methods validate package/attr names against strict
  allowlists before interpolation; `validate_flake_attr` and the plugin
  `validate.rs` metacharacter checks are correct.
- **Concurrent pipe draining is correct** in both `CommandRunner::run`
  (`tokio::join!` before `wait`) and `run_command_sync` (separate threads),
  avoiding the classic pipe-buffer deadlock.
- **The check-epoch guard** in window.rs (`check_epoch`) correctly invalidates
  stale in-flight availability checks.
- **ANSI stripping** (`strip_ansi`) handles CSI and simple escapes and
  deliberately preserves unrecognised sequences.

---

## Suggested remediation order

1. **H1** and **H2** — user-visible, small fixes (broken notifications; wedged UI).
2. **M1** — decide daemon vs. pkexec and delete the unused half; this alone
   removes most of the dead code and the redundant privileged surface.
3. **M2–M4** — either wire up (history/snapshot/config/cleanup/size/changelog)
   or remove; each is a shipped-looking-but-inert feature.
4. **M5, M6** — count/parse correctness.
5. **L*** — hardening and polish, several of which only matter once the daemon
   (M1) is actually used.
