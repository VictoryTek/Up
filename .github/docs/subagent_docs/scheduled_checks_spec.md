# Scheduled Background Checks — Specification

> Feature: Periodic headless update detection via systemd user timer + `notify-send`  
> Status: **Draft — Phase 1 complete**  
> Generated: May 8, 2026

---

## 1. Current State

Update detection in **Up** is entirely in-process and GTK-driven:

- `src/backends/mod.rs` exposes `detect_backends() -> Vec<Arc<dyn Backend>>`, which probes the host for APT / DNF / Pacman / Zypper / Nix / Flatpak / Homebrew / fwupd and returns live backend objects.
- Each `Backend` impl provides `count_available()` (trait default delegates to `list_available().map(|v| v.len())`).
- Detection and counting only happens when the user opens Up and presses the **Check** button.
- `src/runtime.rs` exposes a process-wide shared `tokio::runtime::Runtime` via `OnceLock`.
- `src/main.rs` immediately constructs an `adw::Application` (GTK + display connection) with no CLI argument handling.

There is **no mechanism** to check for updates without the GUI running. The feature tracker in `CODEBASE_ANALYSIS.md` explicitly marks "Scheduled background checks" as unimplemented (`[ ]`).

---

## 2. Problem Statement

Users who leave Up closed will never be notified that their system is out of date unless they manually open the app. A background check that runs on a daily schedule and surfaces a desktop notification resolves this without compromising the daemon-free, on-demand design philosophy.

Requirements:
- Periodic (daily) check with no always-running daemon.
- Desktop notification via standard `notify-send`.
- Opt-in: the user must explicitly enable the timer.
- No GTK window opened during the check.
- No new Cargo dependencies.
- Reuses all existing backend detection and counting infrastructure.

---

## 3. Design Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Scheduling mechanism | systemd user timer | Standard on all systemd-based distros; no daemon; survives reboots via `Persistent=true`; runs in user session with access to `$DISPLAY`/`$DBUS_SESSION_BUS_ADDRESS` |
| Notification mechanism | `notify-send` via `std::process::Command` | Zero new Cargo deps; universally available on GNOME/KDE/XFCE; sufficient for informational nudge |
| Action buttons | Not included | `notify-send` does not support action callbacks; adding `libnotify` or raw D-Bus is out of scope |
| Opt-in vs opt-out | Opt-in (`systemctl --user enable --now`) | Avoids surprising users with unexpected notifications on install |
| Daemon vs one-shot | One-shot (`Type=oneshot`) | No persistent memory/CPU cost; clean process model |
| Duplicate suppression | Stamp file in `$XDG_CACHE_HOME/up/last-check-count` | Avoids repeated identical notifications when the count has not changed |
| Binary invocation | `@BINDIR@/up --check` | Reuses the same binary; `@BINDIR@` substituted by Meson at install time |

---

## 4. Architecture

### 4.1 `--check` CLI Mode in `src/main.rs`

Argument detection must occur **before** any GTK initialisation. The `main()` function currently calls `setlocale`, `bindtextdomain`, `textdomain`, `gio::resources_register_include!`, `env_logger::init()`, and then constructs an `adw::Application`.

The new `--check` guard is inserted as the very first statement of `main()`:

```rust
mod check;  // ← add to module declarations

fn main() -> gtk::glib::ExitCode {
    // Background check mode — must execute before GTK/GIO initialisation.
    // The systemd service unit invokes: up --check
    if std::env::args().any(|a| a == "--check") {
        return check::run_headless_check();
    }

    // i18n — must be before GTK/adw initialization
    setlocale(LocaleCategory::LcAll, "");
    // ... remainder unchanged
}
```

`gtk::glib::ExitCode` is used as the return type to stay consistent with the existing signature. Returning `gtk::glib::ExitCode::SUCCESS` does **not** initialise a GTK display connection.

---

### 4.2 New Module: `src/check.rs`

This module contains the entire headless check pipeline. It must import only standard library, tokio, log, and the existing crate modules — **no GTK types**.

```rust
// src/check.rs

use crate::backends;
use crate::runtime::runtime;
use log::{info, warn};
use std::path::PathBuf;

/// Entry point for `up --check`.
///
/// Detects all available backends, counts pending updates in parallel,
/// compares against the previous run's stamp, and fires a desktop
/// notification if the count has changed and is non-zero.
///
/// Returns `gtk::glib::ExitCode::SUCCESS` unconditionally so that the
/// systemd service unit does not enter a failed state on partial backend
/// errors (e.g. a backend that is unavailable today).
pub fn run_headless_check() -> gtk::glib::ExitCode {
    env_logger::init();

    let backends = backends::detect_backends();

    if backends.is_empty() {
        info!("up --check: no backends detected, exiting");
        return gtk::glib::ExitCode::SUCCESS;
    }

    // Run all count_available() futures concurrently on the shared Tokio runtime.
    let total: usize = runtime().block_on(async {
        let mut set = tokio::task::JoinSet::new();
        for backend in &backends {
            let backend = backend.clone();
            set.spawn(async move {
                match backend.count_available().await {
                    Ok(n) => {
                        info!("up --check: {} reports {} update(s)", backend.display_name(), n);
                        n
                    }
                    Err(e) => {
                        warn!("up --check: {} error: {}", backend.display_name(), e);
                        0
                    }
                }
            });
        }
        let mut sum = 0usize;
        while let Some(res) = set.join_next().await {
            sum += res.unwrap_or(0);
        }
        sum
    });

    info!("up --check: total updates available = {}", total);

    let stamp_path = stamp_file_path();
    let prev_count = read_stamp(&stamp_path);

    if total > 0 && Some(total) != prev_count {
        send_notification(total);
    }

    // Always update stamp so the next run has an accurate baseline.
    write_stamp(&stamp_path, total);

    gtk::glib::ExitCode::SUCCESS
}

// ── Stamp file ────────────────────────────────────────────────────────────────

/// `$XDG_CACHE_HOME/up/last-check-count`  (fallback: `$HOME/.cache/up/…`)
fn stamp_file_path() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))
                .join(".cache")
        });
    base.join("up").join("last-check-count")
}

/// Returns `Some(n)` if a valid stamp exists, `None` otherwise.
fn read_stamp(path: &PathBuf) -> Option<usize> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Atomically writes `count` to the stamp file, creating parent dirs as needed.
fn write_stamp(path: &PathBuf, count: usize) {
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            warn!("up --check: could not create cache dir {}: {}", parent.display(), e);
            return;
        }
    }
    if let Err(e) = std::fs::write(path, count.to_string()) {
        warn!("up --check: could not write stamp file {}: {}", path.display(), e);
    }
}

// ── Notification ──────────────────────────────────────────────────────────────

fn send_notification(count: usize) {
    let summary = if count == 1 {
        "1 update available".to_string()
    } else {
        format!("{} updates available", count)
    };
    let body = "Open Up to review and apply updates.";

    let status = std::process::Command::new("notify-send")
        .args([
            "-a", "Up",
            "-i", "io.github.up",
            "-u", "normal",
            &summary,
            body,
        ])
        .status();

    match status {
        Ok(s) if s.success() => info!("up --check: notification sent ({} updates)", count),
        Ok(s) => warn!("up --check: notify-send exited with status {}", s),
        Err(e) => warn!("up --check: could not spawn notify-send: {}", e),
    }
}
```

**Key properties of this implementation:**
- Uses only `tokio::task::JoinSet` — already a dependency, no new crate.
- Calls `runtime().block_on(...)` — the `OnceLock` runtime from `src/runtime.rs`.
- Error from any single backend is logged at `warn!` level and contributes 0 to the count — the check does not abort.
- `notify-send` failure is non-fatal (missing on TTY sessions, servers, etc.).

---

### 4.3 New Data File: `data/io.github.up-check.service.in`

Template file. Meson's `configure_file()` replaces `@BINDIR@` with the installed binary directory.

```ini
[Unit]
Description=Up — background update check
Documentation=https://github.com/VictoryTek/Up

[Service]
Type=oneshot
ExecStart=@BINDIR@/up --check
```

Notes:
- `Type=oneshot` — process starts, runs check, exits. No persistent state.
- No `User=` directive — systemd runs user units under the invoking user's UID automatically.
- No `Environment=` needed — systemd user instances inherit `XDG_RUNTIME_DIR`, `DBUS_SESSION_BUS_ADDRESS`, and `DISPLAY`/`WAYLAND_DISPLAY` when the user is logged in. For systems with user lingering enabled (`loginctl enable-linger`), `XDG_RUNTIME_DIR` is set by pam_systemd even without an active session.

---

### 4.4 New Data File: `data/io.github.up-check.timer`

No substitution needed; this file is installed as-is.

```ini
[Unit]
Description=Up — daily update check timer
Documentation=https://github.com/VictoryTek/Up

[Timer]
OnCalendar=daily
Persistent=true
RandomizedDelaySec=30min

[Install]
WantedBy=timers.target
```

Notes:
- `OnCalendar=daily` — fires at midnight; combined with `RandomizedDelaySec` this spreads load.
- `Persistent=true` — if the system was asleep or off at the scheduled time, the timer fires immediately at next wake/boot (up to once per day).
- `RandomizedDelaySec=30min` — prevents thundering-herd when many systems boot simultaneously.
- `WantedBy=timers.target` — standard target for user timers; pulled in by `multi-user.target` in user sessions.

---

### 4.5 Changes to `meson.build`

Two new blocks, inserted after the existing `install_data('data/io.github.up.policy', ...)` block:

```meson
# ── Systemd user units ────────────────────────────────────────────────────────
# Determine the correct install directory.
# Prefer the pkgconfig variable from the systemd dependency (accurate for
# split-usr systems); fall back to the FHS default.
systemd_dep = dependency('systemd', required: false)
if systemd_dep.found()
  systemd_user_unit_dir = systemd_dep.get_variable(pkgconfig: 'systemduserunitdir')
else
  systemd_user_unit_dir = join_paths(prefix, 'lib', 'systemd', 'user')
endif

# Substitute @BINDIR@ → the installed binary path in the service file.
configure_file(
  input: 'data/io.github.up-check.service.in',
  output: 'io.github.up-check.service',
  configuration: {'BINDIR': bindir},
  install: true,
  install_dir: systemd_user_unit_dir,
)

install_data('data/io.github.up-check.timer',
  install_dir: systemd_user_unit_dir,
)
```

The `bindir` variable is already defined earlier in `meson.build` as:
```meson
bindir = join_paths(prefix, get_option('bindir'))
```

---

### 4.6 Duplicate Notification Suppression (Stamp File)

The stamp file strategy prevents the daily timer from re-notifying the user about the same N pending updates every day:

```
Run N:  count=5, stamp=None  → notify("5 updates available"), write stamp=5
Run N+1: count=5, stamp=5   → silent (count unchanged)
Run N+2: count=7, stamp=5   → notify("7 updates available"), write stamp=7
Run N+3: count=0, stamp=7   → silent (count is 0), write stamp=0
Run N+4: count=5, stamp=0   → notify("5 updates available"), write stamp=5
```

This means:
- Users are notified when the count **first becomes non-zero** or **increases/decreases to a new non-zero value**.
- Once the count drops to 0 (system updated), the stamp resets so the next cycle can notify again.
- If the user ignores the notification for weeks, they are **not** re-notified daily about the same updates.

---

## 5. Files to Create / Modify

| Path | Action | Description |
|---|---|---|
| `src/check.rs` | **Create** | Headless check module: backend detection, parallel counting, stamp logic, notify-send |
| `src/main.rs` | **Modify** | Add `mod check;` declaration; detect `--check` arg before GTK init |
| `data/io.github.up-check.service.in` | **Create** | systemd service template with `@BINDIR@` placeholder |
| `data/io.github.up-check.timer` | **Create** | systemd timer unit (daily, persistent) |
| `meson.build` | **Modify** | Add `systemd_dep` detection, `configure_file()` for service, `install_data()` for timer |

---

## 6. Implementation Steps

### Step 1 — Create `src/check.rs`
Create the file as shown in §4.2. Verify that it compiles cleanly against the existing `backends::detect_backends()` and `runtime::runtime()` APIs.

### Step 2 — Modify `src/main.rs`
1. Add `mod check;` to the module declarations block (after `mod ui;`).
2. Insert the `--check` guard as the first statement in `main()`, before `setlocale(...)`.

### Step 3 — Create `data/io.github.up-check.service.in`
Create the file with content exactly as shown in §4.3. The `@BINDIR@` token must match the `configuration` key in the Meson `configure_file()` call exactly.

### Step 4 — Create `data/io.github.up-check.timer`
Create the file with content exactly as shown in §4.4.

### Step 5 — Modify `meson.build`
Insert the two new blocks from §4.5 after the existing `install_data` block for `io.github.up.policy`.

### Step 6 — Build Validation
```bash
cargo build                        # must compile without errors
cargo clippy -- -D warnings        # must produce no warnings
cargo fmt --check                  # must pass
cargo test                         # all tests must pass
```

For Meson integration:
```bash
meson setup builddir               # must succeed; confirm unit dir is detected
meson compile -C builddir          # must succeed
```

### Step 7 — Manual Smoke Test (Linux only)
```bash
# Build and run in check mode
cargo build
./target/debug/up --check          # should exit cleanly; check $HOME/.cache/up/last-check-count
cat $HOME/.cache/up/last-check-count

# Verify a second run is silent (stamp matches)
./target/debug/up --check          # should not fire notify-send again

# Simulate stamp mismatch
echo "999" > $HOME/.cache/up/last-check-count
./target/debug/up --check          # should fire notify-send if any updates are available
```

---

## 7. Dependencies

**No new Cargo dependencies are required.**

All components used by `src/check.rs` are already in scope:

| Component | Source | Already in `Cargo.toml`? |
|---|---|---|
| `backends::detect_backends()` | `src/backends/mod.rs` | ✅ (internal) |
| `runtime::runtime()` | `src/runtime.rs` | ✅ (internal) |
| `tokio::task::JoinSet` | `tokio` crate | ✅ `tokio = { features = ["rt-multi-thread", …] }` |
| `log::info!`, `log::warn!` | `log` crate | ✅ |
| `env_logger::init()` | `env_logger` crate | ✅ |
| `std::process::Command` | Rust standard library | ✅ |
| `std::fs::read_to_string`, `std::fs::write`, `std::fs::create_dir_all` | Rust standard library | ✅ |

The `notify-send` binary is a runtime dependency only (not a Cargo dep). It is provided by:
- `libnotify-bin` on Debian/Ubuntu
- `libnotify` on Fedora/Arch
- Not required to be present — failures are logged at `warn!` level and ignored.

---

## 8. Risks & Mitigations

### R1 — Nix backend always reports 0 updates
**Risk:** `NixBackend::list_available()` returns `Ok(vec![])` by design (NixOS cannot enumerate available updates without running a full rebuild). The `count_available()` default therefore always returns 0 for Nix.  
**Impact:** Users on pure NixOS will never receive a background notification for OS-level updates.  
**Mitigation:** Accept as a known limitation; document in the UI/README. Nix users are expected to be more hands-on. Firmware (fwupd), Flatpak, and Homebrew will still generate notifications on NixOS if installed. A future `count_available()` override for `NixBackend` using `nix-env -qa --compare-versions` or a channel diff could be added separately.

### R2 — `notify-send` unavailable or display not set
**Risk:** In lingering sessions without an active graphical display, `notify-send` will fail.  
**Mitigation:** Failure is already handled gracefully — `send_notification()` logs a `warn!` and returns without aborting. The stamp file is still written so the next successful run won't re-trigger.

### R3 — `XDG_RUNTIME_DIR` not set for linger sessions
**Risk:** On systems without PAM session configuration for lingering, `XDG_RUNTIME_DIR` may not be set, which can cause D-Bus-dependent backends (fwupd) to fail.  
**Mitigation:** fwupd backend errors contribute 0 to the count (not fatal). `notify-send` itself uses D-Bus; if D-Bus is unavailable the notification silently fails. Linger support requires `loginctl enable-linger $USER` and a properly configured `pam_systemd`.

### R4 — Timer fires while Up is already running an update
**Risk:** `count_available()` calls (e.g. `apt list --upgradable`) may race with an in-progress `apt upgrade` holding the dpkg lock, causing the check to block or fail.  
**Mitigation:** Each backend's `count_available()` is already wrapped to return `Err(_)` on non-zero exit, which maps to 0 in the check sum. A transient dpkg lock error is indistinguishable from "no updates"; the next daily run will re-evaluate.

### R5 — Binary path mismatch if installed to non-standard prefix
**Risk:** If the user installs Up to `$HOME/.local` via Meson, `@BINDIR@` will be `$HOME/.local/bin/up`. The service unit will correctly reference this path, but the unit directory (`$prefix/lib/systemd/user`) must also be within `$HOME/.local` for user-scoped installation to work.  
**Mitigation:** This is standard Meson behaviour for user-prefix installs. Document in `README.md` that enabling the timer requires: `systemctl --user daemon-reload && systemctl --user enable --now io.github.up-check.timer`.

### R6 — `--check` flag conflicts with future GTK command-line options
**Risk:** `adw::Application` / `gtk::Application` registers its own flags via GIO's command-line handling. If GTK is initialised before our check, GLib may parse (and reject) `--check` as an unknown option.  
**Mitigation:** Fully mitigated by the design: the `--check` guard runs before `adw::Application::builder().build()`. GLib's option parser is never invoked.

### R7 — Flatpak sandbox prevents `notify-send` and `systemd --user`
**Risk:** If Up is distributed as a Flatpak, the sandbox blocks `notify-send` (requires `--talk-name=org.freedesktop.Notifications`) and the systemd user units cannot be installed inside the sandbox.  
**Mitigation:** Per `CODEBASE_ANALYSIS.md`, Flatpak distribution is **retired**. Up is distributed exclusively via Nix flake. This risk is moot.

---

## 9. User Documentation Notes

The following should be added to `README.md` under a new **Scheduled Checks** section:

```markdown
## Scheduled Update Checks

Up can check for updates daily in the background and send a desktop
notification when new updates are available.

This feature uses a systemd user timer and is **opt-in**. After installing Up,
enable it with:

    systemctl --user enable --now io.github.up-check.timer

To disable:

    systemctl --user disable --now io.github.up-check.timer

The timer fires once daily. If your system was off or asleep at the scheduled
time, the check runs automatically at next login (via `Persistent=true`).

Notifications are suppressed if the update count has not changed since the
last check. The count is stored in `$XDG_CACHE_HOME/up/last-check-count`.
```

---

## 10. Summary

This feature requires:

- **1 new Rust module** (`src/check.rs`, ~80 lines)
- **2 new data files** (`data/io.github.up-check.service.in`, `data/io.github.up-check.timer`)
- **2 small file edits** (`src/main.rs` — 4 lines; `meson.build` — ~12 lines)
- **0 new Cargo dependencies**

All backend detection, parallel async execution, and runtime infrastructure already exist. The feature is entirely additive and does not alter any existing runtime paths.
