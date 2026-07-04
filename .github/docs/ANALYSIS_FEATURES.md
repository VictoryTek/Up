# Up — Feature Opportunity Analysis

What's worth building next, based on what the code already contains. The
striking pattern in this codebase: **at least eight features are fully or
mostly implemented at the module level but never wired into the UI or entry
point.** These are the cheapest wins by far — the risky logic (subprocess
handling, parsing, privilege escalation) already exists and is unit-tested;
what's missing is usually 20–100 lines of glue in `window.rs` or `main.rs`.

Priorities weigh effort against value. "Effort" assumes no rearchitecture —
everything below fits the existing async-channel/orchestrator/backend-trait
structure.

---

## Tier 1 — Finish what's already built (high value, low effort)

### 1. Background update checks + desktop notifications — **HIGH**
**What already exists:** [src/check.rs](src/check.rs) is a complete
implementation: detects backends, counts pending updates via
`count_available()`, keeps a stamp file in `$XDG_CACHE_HOME/up/` to avoid
duplicate notifications, and fires `notify-send` with the app icon. The systemd
units are already packaged: [data/io.github.up-check.timer](data/io.github.up-check.timer)
(daily) and [data/io.github.up-check.service.in](data/io.github.up-check.service.in)
which runs `up --check`.

**What's missing:** `main()` never parses arguments, so `--check` is handed to
GTK and rejected. The whole feature is one `if` away from working.

**Concretely:** In [src/main.rs](src/main.rs), before constructing
`UpApplication`, check `std::env::args().any(|a| a == "--check")` and call
`check::run_check()` + return (also remove the duplicate `env_logger::init()`
in one of the two places). Then remove the `#![allow(dead_code)]` from
check.rs. Optionally add a `--check` handler via
`gio::ApplicationFlags::HANDLES_COMMAND_LINE` if you want GApplication-native
option handling, but the pre-GTK early-exit is simpler and avoids initializing
a display in a timer context.

**Why users expect it:** Every comparable tool (GNOME Software, Discover,
pamac) notifies about pending updates. The timer already ships — right now it
just logs an error every day.

---

### 2. Update History page — **HIGH**
**What already exists:** A complete storage layer
([src/history.rs](src/history.rs): JSONL append/load/clear with XDG paths) and
a complete UI ([src/ui/history_page.rs](src/ui/history_page.rs): populated
list rows with per-result icons, timestamps via `glib::DateTime`, and a Clear
button already wired to `clear_history()`). Both are dead code today.

**What's missing:** Two call sites:
1. In [src/ui/window.rs](src/ui/window.rs) `UpWindow::build()`, add a third
   ViewStack page: `view_stack.add_titled_with_icon(&HistoryPage::build(),
   Some("history"), "History", "document-open-recent-symbolic")`.
2. In the `OrchestratorEvent::BackendFinished` handler (window.rs, both the
   Update-All loop and the retry loop), map `UpdateResult` →
   `history::HistoryEntry` and call `append_entry()`. The `result` string
   values the page expects (`"success"`, `"success_self_update"`, `"error"`,
   `"skipped"`) are already defined by the page's `match`.

**One design decision:** the History page currently loads entries at build
time only; either rebuild rows when the page becomes visible
(`connect_visible_child_notify` on the stack) or re-populate after each
`AllFinished`.

---

### 3. Persisted preferences (skip choices survive restart) — **HIGH**
**What already exists:** [src/config.rs](src/config.rs) has
`AppConfig { skipped_backends: Vec<BackendKind>, snapshot_preference }` with
JSON load/save, XDG paths, and serde on `BackendKind` — and zero callers. The
skip checkboxes in [src/ui/update_row.rs](src/ui/update_row.rs) work but reset
every launch.

**Concretely:** In window.rs after backend detection completes, call
`config::load_config()` and pre-set `skip_checkbox` (add a
`set_skipped(bool)` method to `UpdateRow`). In the existing
`on_skip_changed` callback, collect the currently-skipped kinds from `rows`
and `save_config()`. ~40 lines total. This also makes `snapshot_preference`
loadable for feature #5.

**Why users expect it:** Unchecking "Flatpak" every single launch is the kind
of paper-cut that gets apps uninstalled.

---

### 4. Cleanup / maintenance mode — **HIGH**
**What already exists:** Every backend implements `run_cleanup()` with real
logic — `apt autoremove`, `dnf autoremove`, pacman orphan removal via
`-Qtdq`/`-Rns`, zypper orphan parsing, `flatpak uninstall --unused`,
`brew autoremove` + `cleanup`, `nix-collect-garbage -d` — plus
`supports_cleanup()` flags and a finished
[`CleanupOrchestrator`](src/orchestrator.rs#L209-L274) that reuses the whole
event/auth/log pipeline. The daemon even has `run_cleanup` D-Bus methods and a
`io.github.up.cleanup.system` polkit action. None of it is reachable.

**Concretely:** Add a "Clean Up" entry to the header-bar menu (next to "About
Up") or a secondary button in the hero row. Clicking it runs
`CleanupOrchestrator::run_all()` over `detected` backends filtered by
`supports_cleanup()`, driving the same `rows`/`log_panel`/`progress_bar`
handlers the update flow uses (the event enum is shared, so the existing
match arms mostly work as-is). Show "N removed" via the existing
`set_status_success(count)`.

**Why users expect it:** "Remove unused packages / free disk space" is a
standard companion to "update everything" (pamac, Discover, brew users run
cleanup habitually).

---

### 5. Pre-update snapshots (Timeshift/Snapper/btrfs) — **MEDIUM**
**What already exists:** [src/snapshot.rs](src/snapshot.rs) fully implements
detection (Snapper config check, Timeshift config check, btrfs root +
`/.snapshots`) and creation via `pkexec` for all three tools.
`config::SnapshotPreference { Ask | Always | Never }` exists for exactly this
flow. The daemon has `create_snapshot` methods, allowlisted timeshift/snapper
commands, and the `io.github.up.snapshot.create` polkit action.

**Concretely:** In the Update-All click handler (window.rs), after the
metered/battery gates and before `orchestrator.run_all()`:
`detect_snapshot_tool()` on a background task; if `Some(tool)` and preference
is `Ask`, show an `adw::AlertDialog` ("Create a system snapshot first?" /
Skip / Snapshot & Update / checkbox "remember my choice" → writes
`snapshot_preference`); on confirm, `create_snapshot(tool).await`, streaming a
line into the log panel, then proceed. Failure should warn but offer to
continue.

**Value:** This is the single biggest trust feature for a tool that runs
`pacman -Syu --noconfirm` on people's systems. Effort is moderate only because
of dialog-flow plumbing; the dangerous parts are done.

---

### 6. Show download/disk sizes next to pending updates — **MEDIUM**
**What already exists:** `estimate_size()` is implemented for APT
(`apt-get -s upgrade`), DNF (`--assumeno`), Zypper (`--dry-run`), Flatpak
(`remote-ls --columns=download-size`), fwupd (JSON `Size`), and plugins
(`size_regex` parser). [src/disk.rs](src/disk.rs) has tested parsers plus
`format_bytes()` and `detect_available_space()`. No caller anywhere.

**Concretely:** In the `run_checks` closure (window.rs), alongside the
existing `count_available()`/`list_available()` background call, also invoke
`backend.estimate_size()`; extend the channel payload; render it in the row
status — `"7 available · ~132 MB"` — and sum into the hero status line
(`"12 updates available (~450 MB)"`). Bonus: compare the sum against
`disk::detect_available_space()` and reuse the metered-style banner to warn
when disk is low, mirroring the upgrade page's 10 GB check.

---

### 7. Selective updates — make the per-package checkboxes real — **MEDIUM**
**What already exists:** The entire backend and orchestration layer:
`supports_item_selection()` (true for APT, DNF, Zypper, Flatpak, Homebrew, and
non-VexOS NixOS flakes), `run_selected_update()` with per-backend name
validation, and the orchestrator dispatch
([orchestrator.rs:157-162](src/orchestrator.rs#L157-L162)) that routes
`Some(items)` correctly. The UI already lists pending package names inside
each `ExpanderRow` (`set_packages`). But window.rs always passes `(backend,
None)`, so selection is unreachable.

**Concretely:** In `UpdateRow::set_packages`, when the backend
`supports_item_selection()`, build each package row with a `gtk::CheckButton`
suffix (default checked) and track `Rc<RefCell<HashMap<String, bool>>>`; add
`UpdateRow::selected_items() -> Option<Vec<String>>` returning `None` when all
are checked (meaning "full update" — important, since a full `apt upgrade`
resolves dependencies better than an explicit list). In the Update-All
handler, replace `.map(|b| (b, None))` with a lookup of the row's
`selected_items()`.

**Priority note:** Medium rather than high because the honest version needs
care around the 50-item display cap in `set_packages` (selection must not
silently exclude the "…and N more" remainder) and around dependency
implications. But the scary half — safe privileged execution of a package
subset — is done and validated.

---

### 8. Changelog / "What's new" viewer — **MEDIUM**
**What already exists:** [src/changelog.rs](src/changelog.rs) — 250 lines,
complete: per-backend fetchers (`apt-cache show`, `dnf updateinfo info`,
`pacman -Si`, `zypper info`, flatpak `remote-info --log` with sandbox-aware
remote mapping, `brew info`, fwupd JSON release descriptions), 30s timeouts,
10 KB truncation. Zero callers. A `changelog_viewer_spec.md` exists in
`.github/docs/subagent_docs/`, so this was explicitly planned.

**Concretely:** Add a small "document" icon button as a suffix on each
`UpdateRow` (visible when `last_available_count() > 0`). On click, spawn
`fetch_changelog(kind, &packages)` on the runtime, then present an
`adw::Dialog` with a monospace `gtk::TextView` inside a `ScrolledWindow`
(the LogPanel already demonstrates the exact widget recipe). Handle
`ChangelogError::NotSupported` by hiding the button for Nix/plugins.

---

## Tier 2 — Natural complements to the existing structure

### 9. Adopt the daemon for privileged execution (or delete it) — **MEDIUM**
**What already exists:** A complete privileged D-Bus service
([daemon/](daemon/)): polkit checks per operation, a fixed command allowlist,
audit logging to the journal, process-group SIGTERM/SIGKILL cancellation, idle
auto-exit, systemd + D-Bus activation files, and a matching client
([src/dbus_client.rs](src/dbus_client.rs)) with `detect_execution_mode()`
fallback to pkexec. None of it is used; the GUI drives a long-lived
`pkexec /bin/sh` instead.

**Why it's worth finishing rather than deleting:**
- **Real mid-command cancel.** Today `CancelHandle` can only close the root
  shell's stdin, so a hung `apt` keeps running; the daemon kills the process
  group. The Cancel button becomes honest.
- **Tighter security.** A fixed allowlist replaces a live root shell fed by
  string-built commands, and it closes the plugin `pkexec`-routing hole noted
  in ANALYSIS_BUGS.md.
- **Better UX.** `auth_admin_keep` on the daemon actions means one polkit
  prompt covers update + cleanup + snapshot in a session.

**Concretely:** In the orchestrator's auth phase, call
`detect_execution_mode()`; when `Daemon`, hand backends a
`DaemonExecutor` instead of `CommandRunner` (both implement
`CommandExecutor` already — that trait was clearly designed for this exact
swap). Fix the client's known issues first (subscribe to signals *before*
calling `run_update`; handle stream termination in the `select!`). The daemon
needs its allowlist extended to match what backends actually run (the flake
`nixos-rebuild` invocations, `vexos-update`), and `run_upgrade` needs its
(currently empty) command table populated.

If this isn't wanted, the counter-recommendation stands: remove the daemon
crate and packaging. Shipping an unused privileged service is the worst of
both worlds.

### 10. Plugin manager UI + ship the existing plugin descriptors — **MEDIUM**
**What already exists:** A full plugin pipeline — XDG-dir discovery, 64 KB
size cap, schema/security validation, the `.disabled` override mechanism in
`/etc/up/backends.d/`, and four ready descriptors: `data/backends.d/apk.yaml`
and `xbps.yaml` (Alpine, Void) plus `examples/plugins/eopkg.yaml` and
`swupd.yaml` (Solus, Clear Linux). The descriptors even define `cleanup` and
`estimate_size` commands that features #4 and #6 would light up.

**Concretely:** (a) Install `data/backends.d/*.yaml` to
`/usr/share/up/backends.d/` in the meson/packaging step so Alpine and Void
users get support out of the box. (b) Add a "Plugins" section to a new
preferences dialog listing `discover_plugins()` results (name, version,
author, source path) with a toggle that, for user-dir plugins, renames the
file to `.yaml.disabled` (the admin-level `.disabled` flag requires root and
can stay documentation-only). This turns an invisible power feature into a
visible extension story.

### 11. Failure details on click — **MEDIUM** (small effort)
**What already exists:** Errors land in a single ellipsized `gtk::Label` on
the row (`set_status_error`), while the full context scrolls away in the
shared log panel. `CommandRunner` already tracks a 100-line output tail per
command precisely so errors have context — but that tail is discarded on the
error path (`PrivilegedShell::run_command` returns only
`"Command exited with code N"`).

**Concretely:** Include the retained tail in `BackendError::Exit::message`
(runner.rs already has `tail_str` in scope at the failure return), store the
last result on `UpdateRow`, and make the error label a button that opens a
dialog with the message + tail. Turns "Error: Command failed (exit 100)" into
something actionable.

### 12. Configurable battery/metered gates — **LOW**
**What already exists:** [src/battery.rs](src/battery.rs) sysfs reader, a
hardcoded `capacity < 40` threshold, the `gio::NetworkMonitor` metered check,
and per-run `bypass_*` cells. With config.rs wired (feature #3), adding
`battery_threshold: u8` and `warn_on_metered: bool` to `AppConfig` plus two
rows in a preferences dialog is trivial. (While there: filter
`/sys/class/power_supply` entries by `scope != Device` so a wireless mouse at
5% doesn't trigger the low-battery warning.)

### 13. Auto-recheck when VexOS binary cache is syncing — **LOW**
**What already exists:** `UpdateResult::CacheMiss` (exit-2 detection from
`vexos-update`) with a dedicated row status "Binary cache syncing, try again
later", and an epoch-guarded `run_checks` closure that's safe to re-trigger.

**Concretely:** When a run finishes with any `CacheMiss` result, schedule
`glib::timeout_add_seconds_local` (e.g. 15 min, once) to re-run `run_checks`
and update the hero label — "will re-check at 14:32". Small, targeted QoL for
the distro this app clearly ships on.

---

## Tier 3 — Expected-but-missing (bigger or lower urgency)

### 14. Finish localization — **MEDIUM**
**What already exists:** gettext is a dependency; `upgrade_page.rs`,
`history_page.rs`, and `reboot_dialog.rs` wrap strings in `gettext()`;
`po/POTFILES.in` lists the sources; `po/LINGUAS` is an empty scaffold; a
`localization_spec.md` exists in the docs. But `window.rs`, `update_row.rs`,
and `log_panel.rs` — the primary UI — use raw string literals ("Update All",
"Checking for updates...", "Everything is up to date.", etc.), and nothing in
`main.rs` calls `gettextrs::bindtextdomain`/`textdomain`, so even the wrapped
strings never translate.

**Concretely:** Add the `bindtextdomain`/`bind_textdomain_codeset`/`textdomain`
init in `main()`, wrap the remaining user-visible strings (POTFILES already
lists the right files), and the infrastructure is done — translations become a
community contribution surface.

### 15. Flatpak packaging + working self-update banner — **MEDIUM**
**What already exists:** Extensive sandbox support that currently has no
consumer: `flatpak-spawn --host` routing throughout (`flatpak.rs`, `nix.rs`,
`reboot.rs`, `changelog.rs`), `is_running_in_flatpak()`,
`UpdateResult::SuccessWithSelfUpdate` + the restart banner in window.rs, and
five FLATPAK_CI documents in `.github/docs/`. The README explicitly says
"Flatpak packaging … planned for a future release."

**Concretely:** This is packaging work, not app code: write the Flathub
manifest (the CI docs already sketch it), grant `--talk-name=org.freedesktop.Flatpak`
for `flatpak-spawn`, and the dormant sandbox pathways — including the
self-update restart banner — activate as-is.

### 16. Update the README feature matrix as these land — **LOW**
The README already promises "prerequisite checks," lists distro support, and
undersells what's in the tree (no mention of fwupd, plugins, Homebrew
cleanup, VexOS). Cheap credibility win; also the Architecture section
references `upgrade.rs` which is now a directory — stale.

---

## What I deliberately did NOT recommend

- **Offline/A-B updates, package downgrade/rollback UI, or a full package
  browser** — genuinely valuable but each is a rearchitecture (transaction
  staging, version pinning per backend) rather than an extension of what
  exists.
- **A tray/status icon** — GNOME has no sanctioned tray; the systemd timer +
  notification path (#1) is the platform-correct equivalent.
- **Flatpak per-app permission management** — out of scope for an updater.

## Suggested build order

| # | Feature | Priority | Rough effort |
|---|---------|----------|--------------|
| 1 | `--check` wiring (notifications) | High | Hours |
| 3 | Persisted preferences | High | Hours |
| 2 | History page wiring | High | ~1 day |
| 4 | Cleanup mode UI | High | 1–2 days |
| 11 | Error details dialog | Medium | ~1 day |
| 6 | Size estimates in rows | Medium | 1–2 days |
| 8 | Changelog viewer | Medium | 1–2 days |
| 5 | Pre-update snapshots | Medium | 2–3 days |
| 7 | Real package selection | Medium | 2–3 days |
| 10 | Plugin manager + ship descriptors | Medium | 2–3 days |
| 9 | Daemon adoption (or removal) | Medium | ~1 week (or 1 day to remove) |
| 14 | Localization completion | Medium | 1–2 days |
| 15 | Flatpak packaging | Medium | packaging effort |
| 12, 13, 16 | Config gates, cache recheck, README | Low | hours each |

Items 1–4 together would take roughly a week and convert four dead modules
into shipped features with almost no new risky code.
