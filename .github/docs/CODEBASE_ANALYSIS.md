# Up — Codebase Analysis

> Generated: May 6, 2026. Read-only deep-dive. No files were modified during analysis.

---

## Executive Summary

- **Flatpak distribution retired.** Up is distributed exclusively via Nix flake. All Flatpak manifest, CI, and packaging scripts (`io.github.up.json`, `build-flatpak.sh`, `flatpak-ci.yml`) are out of scope. `cargo-sources.json` at the repo root is an orphan and should be deleted.
- **Wrong upstream URL — FIXED.** `Cargo.toml` and `data/io.github.up.metainfo.xml` now correctly reference `VictoryTek/Up`.
- **Persistent privileged `pkexec sh` shell** in `src/runner.rs` uses a stdout sentinel (`___UP_RC_<n>___`) that any spawned subprocess can spoof, causing Up to misreport exit status. No per-command timeout, no cancellation, no SIGINT forwarding.
- **Significant code duplication**: `validate_hostname` exists in both `upgrade.rs` and `nix.rs`; availability-check and `pkexec` scaffolding are repeated across every backend with subtle parsing differences.
- **Hidden panics**: `.expect("distro info must be available …")` in `src/ui/upgrade_page.rs` and `rows.borrow()[idx]` in `src/ui/window.rs` can panic the GTK main loop.
- **Fake progress UI — FIXED.** `src/ui/update_row.rs` fake `ProgressBar` + 200 ms timer removed; `gtk::Spinner` is now the sole in-progress indicator.
- **Two parallel command-execution code paths** — `tokio::process::Command` in `runner.rs` vs `std::process::Command` in `upgrade.rs` — no shared abstraction, divergent error reporting.
- **No timeouts, no cancellation** anywhere. A stuck `apt`/`dnf` waiting on a dpkg lock will hang the UI button forever.
- **Versioning — FIXED.** `meson.build` and `flake.nix` now auto-source the version from `Cargo.toml` at configure/eval time; no more hand-sync.

---

## Progress Tracker

### 1. Quick Fixes (URLs, dead code, docs)
- [x] Fix placeholder URL in `Cargo.toml` (`repository = "https://github.com/user/up"`)
- [x] Fix placeholder URLs in `data/io.github.up.metainfo.xml` (homepage + bugtracker)
- [x] Remove or wire up `CheckMsg::Error` dead code in `src/ui/upgrade_page.rs`
- [x] Remove unused `gettext` / `libunwind-dev` from `.github/workflows/ci.yml`
- [x] Fix `cargo test --release` double-compile in CI (drop `--release` from test step)
- [x] Reconcile Flatpak docs: either ship the manifest/scripts/workflow or rewrite `docs/FLATPAK_CI_SUMMARY.md`

### 2. Bugs & Risks
- [x] **[HIGH]** Harden `PrivilegedShell` stdout-sentinel: reject `\n`/`\0` in args at minimum (`src/runner.rs`)
- [x] **[HIGH]** Add per-command timeout to `PrivilegedShell::run_command` and surface pkexec 126/127 as auth-cancelled
- [x] **[HIGH]** Replace `rows.borrow()[idx]` index lookup with lookup-by-`BackendKind` or pass row clone into closure (`src/ui/window.rs`)
- [x] **[HIGH]** Replace `.expect("distro info must be available …")` with `if let Some(…) else { return; }` (`src/ui/upgrade_page.rs`)
- [x] **[HIGH]** Fix NixOS/Determinate detection when running inside Flatpak sandbox (`src/backends/nix.rs`)
- [x] **[MED]** Fix Ubuntu upgrade tail-thread leak — use `Arc<AtomicBool>` cancellation flag (`src/upgrade.rs`)
- [x] **[MED]** Fix DNF `count_available` — treat exit 100 as "updates available", not any non-zero exit (`src/backends/os_package_manager.rs`)
- [x] **[MED]** Surface `reboot` failures to user (toast on non-zero exit, especially under Flatpak) (`src/reboot.rs`)
- [x] **[MED]** Force `LANG=C` on all subprocess invocations used for parsing (prevents locale-dependent breakage)
- [x] **[MED]** Fix Nix flake target inconsistency — use `resolve_nixos_flake_attr()` in `upgrade_nixos` (`src/upgrade.rs`)
- [x] **[MED]** Fix Flatpak self-update to use a fixed `$XDG_RUNTIME_DIR` temp path instead of predictable `/tmp/up-self-update.flatpak`
- [x] **[MED]** Add cancellation / disable refresh button while an update is in progress (`src/ui/window.rs`)
- [x] **[LOW]** Add `LANG=C` to Zypper `updated_count` parser (counts "done" lines instead of actual packages)
- [x] **[LOW]** Pipe Fedora `dnf system-upgrade reboot` stdout to `tx` instead of `Stdio::null` (`src/upgrade.rs`)
- [x] **[LOW]** Use `flatpak remote-ls --updates` instead of `flatpak update --no-deploy` for list_available (`src/backends/flatpak.rs`) — affects FlatpakBackend (user's system packages)
- [x] **[LOW]** Use `--columns=application` for stable Flatpak column layout (`src/backends/flatpak.rs`)

### 3. Security
- [x] **[HIGH]** Ship `io.github.up.policy` with scoped polkit actions (`update.system`, `upgrade.system`) instead of relying on default `org.freedesktop.policykit.exec` rule
- [x] **[MED]** Pass Flatpak self-update URL as positional bash arg rather than interpolating into script body (`src/backends/flatpak.rs`)
- [x] **[MED]** Add checksum/signature verification for self-update `.flatpak` bundle; or rely solely on Flathub OSTree signing and remove the GitHub-direct path
- [x] **[MED]** Feed inline Python script via stdin rather than `format!` in `fetch_github_latest_release` (`src/backends/flatpak.rs`)
- [x] **[LOW]** Make `shell_quote` always single-quote; remove the "no quoting needed" fast-path whitelist (`src/runner.rs`)
- [x] **[LOW]** Strip ANSI escape sequences in `LogPanel` output for readability (`src/ui/log_panel.rs`)

### 4. Architecture & Code Quality
- [x] **[HIGH]** Introduce `CommandExecutor` trait with `MockExecutor` for testing — unblocks all downstream test work
- [x] **[HIGH]** Replace `Result<_, String>` errors with `thiserror`-derived enums per backend (`BackendError::{AuthCancelled, Spawn, Exit, Parse, Network}`)
- [x] **[MED]** Extract `UpdateOrchestrator` from `src/ui/window.rs` into a non-UI module (`src/orchestrator.rs`)
- [x] **[MED]** Make `Backend::count_available` a trait default that calls `list_available().map(|v| v.len())`; backends override only when cheap-counting is faster
- [x] **[MED]** De-duplicate `validate_hostname` / `validate_flake_attr` into a single `nixos::validate_attr` helper
- [x] **[MED]** Consolidate backend parsers into `pub(crate) fn parse_*(&str) -> Vec<String>` and unit-test against captured fixtures
- [x] **[MED]** Centralise upgrade-page state recomputation into a single `recompute_state()` closure (`src/ui/upgrade_page.rs`)
- [x] **[LOW]** Use single source of truth for backend ordering (remove sort in `window.rs`, trust detection order)
- [x] **[LOW]** Split `src/upgrade.rs` into `upgrade/check.rs`, `upgrade/version.rs`, `upgrade/execute.rs`
- [x] **[LOW]** Use `glib::clone!` macro to reduce verbose `Rc::clone()` chains in UI code

### 5. Performance
- [x] **[MED]** Create a single shared Tokio runtime in `main` instead of one fresh runtime per background spawn (`src/ui/mod.rs`)
- [x] **[MED]** Cap `LogPanel` buffer at ~5000 lines with FIFO eviction (`src/ui/log_panel.rs`)
- [x] **[LOW]** Debounce `scroll_mark_onscreen` to ~50–100 ms instead of per-line (`src/ui/log_panel.rs`)
- [x] **[LOW]** Drop fake progress bar — replaced with `gtk::Spinner` (`src/ui/update_row.rs`)
- [x] **[LOW]** Replace `curl` shell-outs in `upgrade.rs` with `ureq` (removes runtime dep, gives proper timeouts)
- [x] **[LOW]** Use `rt-multi-thread` Tokio feature + a shared runtime instead of per-thread `current_thread` runtimes

### 6. Build / Packaging / CI
- ~~**[CRIT]** Create missing `io.github.up.json` Flatpak manifest~~ — N/A: Flatpak distribution retired; Nix flake is the sole release target
- ~~**[HIGH]** Create missing `scripts/build-flatpak.sh` and `scripts/verify-flatpak.sh`~~ — N/A: Flatpak distribution retired
- ~~**[HIGH]** Create missing `.github/workflows/flatpak-ci.yml`~~ — N/A: Flatpak distribution retired
- [x] **[HIGH]** Create release-tag GitHub Actions workflow with Nix flake artifact upload
- [x] **[MED]** Auto-source version from `Cargo.toml` in `meson.build` and `flake.nix` to eliminate hand-sync
- [x] **[MED]** Fix `meson.build` out-of-tree build hygiene (`build_always_stale: true`, `target/<profile>` clobber)
- [x] **[LOW]** Add `cargo audit` / `cargo deny` and `nix flake check` to `scripts/preflight.sh` and CI
- [x] **[LOW]** Add `rust-toolchain.toml` to pin Rust toolchain for reproducible builds
- [x] **[LOW]** Add `.editorconfig`
- [x] **[LOW]** Delete orphaned `cargo-sources.json` at repo root (leftover from retired Flatpak build)

### 7. New Features (by priority)

#### Quick Wins (Small Effort)
- [x] **Per-backend "skip" checkboxes** — `Skipped` variant already exists; high user value
- [ ] **Cleanup / Maintenance actions** — `apt autoremove`, `dnf autoremove`, `nix-collect-garbage`, `flatpak uninstall --unused`
- [x] **Update history log** — JSONL in `XDG_DATA_HOME/up/history.jsonl` + History tab
- [ ] **Metered-connection warning** — via `gio::NetworkMonitor::is_network_metered()`
- [x] **Reboot-required detection** — read `/var/run/reboot-required`, `dnf needs-restarting -r`, `needrestart`; show banner only when actually required
- [ ] **Battery-aware prompt** — warn before long upgrades when battery < 40%
- [x] **Log export / Copy button** — save log buffer to `~/up-update-<timestamp>.log`
- [ ] **A11y audit** — `set_accessible_label` on icon-only buttons; verify contrast in dark style
- [ ] **Per-backend retry button** — trivial once typed errors exist

#### Medium Effort
- [ ] **Dry-run / Preview mode** — `list_available` infra already in place; one "Preview" button to expand rows without running privileged steps
- [ ] **Cancel running update** — close privileged shell stdin; propagate `Cancelled` to each row
- [ ] **fwupd firmware backend** — `fwupdmgr get-updates` / `fwupdmgr update -y` as a new `Backend` impl
- [ ] **Snapshot integration** — detect Snapper / Timeshift / btrfs root; offer pre-update snapshot
- [ ] **Update changelog viewer** — `apt changelog`, `dnf updateinfo info`, OSTree commit summaries per row
- [ ] **Localization** — `gettext-rs` + `po/` directory
- [ ] **Scheduled background checks** — systemd user timer + `notify-send` (out-of-process; no persistent daemon)
- [ ] **Disk-space pre-check** — surface transaction size from APT/DNF/Flatpak before applying

#### Large / v2.0
- [ ] **D-Bus backend service** — small privileged daemon with scoped polkit actions; eliminates `pkexec sh`, enables proper cancellation and audit logging
- [ ] **Backend plugin/discovery system** — YAML descriptors under `/usr/lib/up/backends.d/` for community-added backends (apk, xbps, eopkg, etc.)

#### Out of Scope
- ~~System tray / always-running daemon~~ — conflicts with stated scope and daemon-free design

---

## Detailed Findings

### 2. Code Inconsistencies

| # | File / Location | Severity | Finding |
|---|---|---|---|
| 2.1 | `Cargo.toml`, `data/io.github.up.metainfo.xml` vs `README.md`, `src/backends/flatpak.rs` | High | `repository`/`<url>` is `github.com/user/up`; everything else references `VictoryTek/Up`. |
| 2.2 | `src/upgrade.rs` ↔ `src/backends/nix.rs` | Medium | Two near-identical hostname/attr validators; comment in `upgrade.rs` explicitly notes the duplication. |
| 2.3 | `src/runner.rs` vs `src/upgrade.rs` | Medium | Two unrelated command-runner implementations. Different stderr handling, different log prefixes, different error-reporting shape. |
| 2.4 | `src/backends/os_package_manager.rs` | Medium | Each backend re-implements its own `count_*` and `list_available` text parsing inline; no shared parser abstraction. APT uses `contains('/')`, Pacman `split_whitespace`, Zypper `starts_with("v ")`, DNF heuristic line filters, Flatpak digit-prefix. |
| 2.5 | `src/backends/os_package_manager.rs` Zypper | Low | `updated_count` uses `lines().filter(|l| l.contains("done")).count()` — matches refresh lines too. |
| 2.6 | `src/backends/homebrew.rs` | Low | Counts upgrades by lines containing `Upgrading`/`Pouring`. Casks vs formulae produce different output. |
| 2.7 | Logging | Low | Mix of `log::info!`/`warn!`/`error!`, `eprintln!`, and direct `tx.send_blocking(...)`. No structured/tracing layer. |
| 2.8 | `src/ui/window.rs` | Low | Two distro-detection consumers sharing a `bounded(1)` channel — a future "rescan" double-fire would deadlock the upgrade page. |
| 2.9 | Async runtime model | Info | Every background spawn creates a fresh `current_thread` Tokio runtime. No shared runtime; inconsistent with `tokio = { features = ["rt"] }` (no `rt-multi-thread`). |
| 2.10 | `Cargo.toml` | Info | `glib`/`gio` declared explicitly even though `gtk4`/`libadwaita` re-export them. Version-skew prone. |

---

### 3. Bugs & Risks

| # | File / Location | Severity | Finding | Suggested Fix |
|---|---|---|---|---|
| 3.1 | `src/runner.rs` `PrivilegedShell::run_command` | High | The `___UP_RC_<n>___` exit-code sentinel is parsed from the command's own stdout stream (stderr merged via `2>&1`). Any subprocess that prints a matching line spoofs exit codes. | Use a second FD for the sentinel, or read `wait()` exit status directly. |
| 3.2 | `src/runner.rs` | High | No per-command timeout, no cancel, no SIGINT forwarding. Stuck `apt` (dpkg lock) hangs the whole UI. | Add `tokio::time::timeout`; surface pkexec exit 126/127 as auth-cancelled vs auth-failed. |
| 3.3 | `src/backends/nix.rs` `is_nixos`, `is_determinate_nix` | ~~High~~ N/A | Flatpak distribution retired; Up will not run inside a Flatpak sandbox. Fix was applied; now moot. | — |
| 3.4 | `src/ui/upgrade_page.rs` | High | `.expect("distro info must be available before check button is sensitive")` panics the GTK main loop. | Replace with `if let Some(distro) = … else { return; }`. |
| 3.5 | `src/ui/window.rs` | High | `rows.borrow()[idx]` — index captured from outer loop; panics if backend list ever mutates between detect and future execution. | Look up by `BackendKind` or pass the `UpdateRow` clone directly into the closure. |
| 3.6 | `src/upgrade.rs` Ubuntu tail thread | Medium | `drop(tail_handle)` does not terminate threads. After every Ubuntu upgrade attempt a thread leaks, tailing `main.log` forever. | Use `Arc<AtomicBool>` cancellation flag. |
| 3.7 | `src/backends/flatpak.rs` self-update | ~~Medium~~ N/A | Flatpak self-update mechanism retired with Flatpak distribution. | — |
| 3.8 | `src/backends/flatpak.rs` `fetch_github_latest_release` | ~~Medium~~ N/A | Flatpak self-update mechanism retired with Flatpak distribution. | — |
| 3.9 | `src/runner.rs` `Self::new` | Medium | Readiness probe has no timeout. If `pkexec` blocks indefinitely (no polkit agent), UI hangs forever with no feedback. | Wrap `read_line` in `tokio::time::timeout`; surface "no PolicyKit agent" diagnostically. |
| 3.10 | `src/reboot.rs` | Medium | `systemctl reboot` is fire-and-forget; failure is logged to stderr but not surfaced to the user. | Capture exit status; show a toast on failure. |
| 3.11 | `src/ui/log_panel.rs` | Medium | `TextBuffer` grows unbounded. A multi-GB Fedora system-upgrade can produce hundreds of thousands of lines → memory bloat and UI sluggishness. | Cap to N lines (delete from head when over budget). |
| 3.12 | `src/upgrade.rs` `check_packages_up_to_date` | Medium | Parses `zypper list-updates` and `apt` output without forcing `LANG=C`; non-English locales emit different prefixes → miscounting. | Lock `LANG=C` for all parsed subprocess output. |
| 3.13 | `src/backends/flatpak.rs` `list_available` | Low | Detection of update rows assumes Flatpak's column layout; column numbering changes between Flatpak versions. Failures are silent. | Use `--columns=application` for stable output. |
| 3.14 | `src/backends/os_package_manager.rs` DNF `count_available` | Low | Treats any non-zero exit as "updates available"; DNF returns 1 for errors and 100 for updates. | Mirror the list path: only treat 100 as updates. |
| 3.15 | `src/upgrade.rs` Fedora `dnf system-upgrade reboot` | Low | Spawned with `Stdio::null()` — output discarded. If `pkexec` auth is cancelled, user sees no error. | Pipe to `tx`. |
| 3.16 | `src/upgrade.rs` `fetch_ubuntu_meta_release` | Low | 10s timeout, no retry. Transient blips fall through to slow `do-release-upgrade -c` fallback. | Optional: add one retry. |
| 3.17 | `src/runner.rs` | Low | First `program == "pkexec"` check uses string equality; callers using a wrapped path (`/usr/bin/pkexec`) bypass the elevated shell and trigger a second polkit prompt. | Resolve via `which` once at startup. |
| 3.18 | `src/ui/window.rs` refresh button | Low | Refresh can be clicked while Update All is in progress — runs `apt list --upgradable` against an in-progress dpkg lock, which can hang. | Disable refresh while updating. |
| 3.19 | `src/upgrade.rs` `upgrade_nixos` Flake path | Low | Uses detected hostname rather than `resolve_nixos_flake_attr()` used for normal updates; inconsistency may target a non-existent attr on VexOS. | Use `resolve_nixos_flake_attr()` here too. |
| 3.20 | `/tmp/up-self-update.flatpak` | ~~Low~~ N/A | Flatpak self-update retired with Flatpak distribution. | — |

---

### 4. Architecture & Code Quality

| # | Area | Severity | Finding | Suggestion |
|---|---|---|---|---|
| 4.1 | `Backend` trait shape | Medium | `count_available` is fully implementable as `list_available().map(|v| v.len())`. Most backends duplicate the body. | Provide as trait default; backends override only when cheap-counting is faster. |
| 4.2 | `BackendKind` enum | Medium | Hard-codes everything; no way to add new backends without modifying every match site. | Move to a registry pattern (`Vec<Box<dyn Backend>>` factory list). |
| 4.3 | Privileged execution | Medium | `pkexec` is the only auth path. No D-Bus / PolicyKit native, no hardening (`NoNewPrivileges`), no audit logging. | Long term: small backend daemon spoken to via D-Bus with proper polkit actions. |
| 4.4 | UI / business-logic separation | Medium | `src/ui/window.rs` hosts orchestration (event-channel state machine, retry/abort gating, backend ordering). Should live in a non-UI module. | Extract `UpdateOrchestrator` into `src/orchestrator.rs`. |
| 4.5 | Upgrade page state | Medium | `Rc<RefCell<…>>` everywhere; manual sensitivity recomputation spread over 3 sites — easy for invariants to drift. | Single `recompute_state()` closure called from every state-changing site. |
| 4.6 | Module size | Low | `src/ui/window.rs` is one ~500-line function; `src/upgrade.rs` mixes detection, network checks, version arithmetic, and execution. | Split: `upgrade/check.rs`, `upgrade/version.rs`, `upgrade/execute.rs`. |
| 4.7 | `unsafe` | Info | None used. Good. |
| 4.8 | TODO/FIXME | Info | None present. Good. |
| 4.9 | Dead code | Low | `CheckMsg::Error` is `#[allow(dead_code)]`. Either wire it up or remove it. |
| 4.10 | Testability | High | No `CommandExecutor` trait → no mocks → essentially zero coverage for backends or the privileged path. | Introduce `trait CommandExecutor` + `MockExecutor`; move parsers to `pub(crate) fn` taking `&str`. |
| 4.11 | Error type | Medium | All errors are `String`. Hard to differentiate auth-cancelled vs network vs exit-nonzero without string matching. | Use `thiserror` enums per backend. |
| 4.12 | Backend ordering | Low | Two ordering authorities: detection comment + `window.rs` `sort_by_key`. | Single source: trust detection order; remove sort in `window.rs`. |

---

### 5. Security

| # | Location | Severity | OWASP | Finding | Mitigation |
|---|---|---|---|---|---|
| 5.1 | `src/runner.rs` `PrivilegedShell` | High | A03 Injection | Any `args` value containing a literal newline followed by a crafted command would be executed as root. Currently safe (compile-time static strings only), but one refactor away from disaster. | Reject `\n`/`\0` in args; pass via `printf '%s\0'` and `xargs -0`; or skip the persistent shell for arbitrary args. |
| 5.2 | `src/runner.rs` `shell_quote` | Medium | A03 | "No quoting needed" fast path. Always single-quoting is safer. | Always single-quote; complexity savings aren't worth the audit burden. |
| 5.3 | `src/backends/flatpak.rs` self-update | ~~Medium~~ N/A | A08 | Flatpak self-update retired with Flatpak distribution. | — |
| 5.4 | `src/backends/flatpak.rs` `fetch_github_latest_release` | ~~Medium~~ N/A | A03 | Flatpak self-update retired with Flatpak distribution. | — |
| 5.5 | `src/upgrade.rs` `parse_os_release` | Low | A04 | `trim_matches('"')` instead of POSIX shell-style unescaping. Root-owned file, so low risk. | Acceptable; document the assumption. |
| 5.6 | `src/upgrade.rs` upgrade shell scripts | Medium | A03 | Several `pkexec sh -c "<format!>"` constructions interpolate detected values. | Prefer `pkexec <prog> <argv>` whenever possible; reserve `sh -c` for genuinely needed shell features. |
| 5.7 | Polkit policy | Medium | A05 Security Misconfiguration | Project does not ship a `.policy` file. Every prompt asks to authorize `/bin/sh` with no scope. | Ship `io.github.up.policy` with explicit, scoped actions. |
| 5.8 | `src/ui/log_panel.rs` | Low | A03 | Subprocess output appended verbatim to GTK `TextBuffer`. Not interpreted as markup — safe. ANSI sequences render as garbage. | Optional: strip ANSI for readability. |
| 5.9 | `/tmp/up-self-update.flatpak` | ~~Low~~ N/A | A01 | Flatpak self-update retired with Flatpak distribution. | — |

---

### 6. Performance

| # | Location | Severity | Finding | Suggestion |
|---|---|---|---|---|
| 6.1 | `src/ui/mod.rs` | Medium | Each refresh spawns N background threads, each with its own fresh single-thread Tokio runtime. | Use a single shared `Runtime` initialised in `main`. |
| 6.2 | `Cargo.toml` tokio features | Low | `rt` only — no `rt-multi-thread`. `current_thread` runtime per-thread wastes one OS thread per concurrent backend. | `rt-multi-thread` + a process-wide runtime. |
| 6.3 | `src/ui/log_panel.rs` | Low | `scroll_mark_onscreen` called for every line — extremely noisy during nix-store fetches (10k+ lines). | Debounce to 50–100 ms. |
| 6.4 | `src/backends/flatpak.rs` `list_available` | Low | Runs `flatpak update --no-deploy -y --user` which contacts every remote and downloads metadata. | Use `flatpak remote-ls --updates` (faster, no full update protocol). |
| 6.5 | `src/upgrade.rs` | ~~Low~~ FIXED | ~~`Command::new("curl")` for upgrade availability adds a runtime dep and pays a process spawn.~~ Replaced with `ureq = "3"`. |
| 6.6 | `src/runner.rs` | Low | `full_output = stdout_output + &stderr_output` reallocates large strings for a 30-min Fedora upgrade. | Stream-only: pass a counter callback; don't accumulate. |
| 6.7 | `src/ui/window.rs`, `src/ui/upgrade_page.rs` | Low | Verbose `Rc::clone()` chains. | Use `glib::clone!` macro. |
| 6.8 | `src/ui/update_row.rs` | Low | 200 ms timer per running row; 7 rows = 35 fps of redraws competing with log panel. | Single shared timer driving all rows, or remove fake progress entirely. |

---

### 7. Build / Packaging / CI

| # | Location | Severity | Finding |
|---|---|---|---|
| 7.1 | Project root | ~~Critical~~ N/A | Flatpak distribution retired. `io.github.up.json` will not be created. |
| 7.2 | `.github/workflows/` | High | No release-tag workflow or artifact upload. Flatpak CI is N/A (distribution retired). |
| 7.3 | `scripts/` | ~~High~~ N/A | `build-flatpak.sh` and `verify-flatpak.sh` will not be created (Flatpak distribution retired). |
| 7.4 | `Cargo.toml` | ~~High~~ FIXED | `repository` now correctly points to `https://github.com/VictoryTek/Up`. |
| 7.5 | `data/io.github.up.metainfo.xml` | ~~High~~ FIXED | `<url type="homepage">` and `bugtracker` now correctly reference `VictoryTek/Up`. |
| 7.6 | `meson.build` | Medium | `cargo_build` shells out to `cargo` and copies from `target/<profile>` from `srcdir`. Bypasses out-of-tree build hygiene; `build_always_stale: true` defeats incremental builds. |
| 7.7 | Version sync | ~~Medium~~ FIXED | `meson.build` reads version via `run_command('grep', …)` at configure time; `flake.nix` uses `builtins.fromTOML`. |
| 7.8 | `scripts/preflight.sh` | Medium | Does not validate `flake.nix` (`nix flake check`), does not run `cargo audit`/`cargo deny`. |
| 7.9 | `.github/workflows/ci.yml` | Low | Installs `libunwind-dev` and `gettext` for no observable use. |
| 7.10 | `.github/workflows/ci.yml` | Low | Builds with `--release` then runs tests with `--release` — two full compiles. |
| 7.11 | `cargo-sources.json` | Low | Present at root; leftover from retired Flatpak build. Should be deleted. |
| 7.12 | No `rust-toolchain.toml` | Low | CI installs latest stable per build; project does not pin Rust toolchain. |
| 7.13 | No `cargo-deny.toml` | Low | Supply-chain checks absent. |
| 7.14 | `data/io.github.up.desktop` | Low | Missing conventional `Version=1.5` Desktop Entry spec key. |

---

## Recommended Backlog (Prioritized)

1. ~~Fix placeholder URLs — `Cargo.toml` and `data/io.github.up.metainfo.xml`~~ ✓
2. ~~Reconcile Flatpak docs~~ — N/A: Flatpak distribution retired; Nix flake is the sole release target ✓
3. ~~Harden `PrivilegedShell` stdout-sentinel; reject `\n` in args at minimum~~ ✓
4. ~~Ship `io.github.up.policy` with scoped polkit actions~~ ✓
5. ~~Introduce `CommandExecutor` trait + `MockExecutor` + parser unit tests per backend~~ ✓
6. ~~Replace `String` errors with `thiserror` enums~~ ✓
7. ~~Cap `LogPanel` buffer; debounce auto-scroll; drop fake progress~~ ✓
8. ~~Sandbox-aware NixOS / Determinate detection~~ ✓
9. ~~Add timeouts + cancellation to all command execution~~ ✓
10. ~~Replace `curl` shell-outs with `ureq`~~ ✓
11. ~~Auto-source version in `meson.build` and `flake.nix`~~ ✓
12. **Ship per-backend skip checkboxes + Preview button** ← NEXT
13. Add `cargo audit` / `cargo deny` / `nix flake check` to preflight + CI
14. Add fwupd backend and reboot-required detection
15. Plan v2.0 D-Bus + polkit-action refactor
