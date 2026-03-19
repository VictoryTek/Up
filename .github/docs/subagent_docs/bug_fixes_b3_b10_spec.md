# Bug Fixes B3–B10: Research & Specification

**Project:** Up — GTK4/libadwaita Linux desktop updater/upgrader  
**Language:** Rust (Edition 2021)  
**Date:** 2026-03-18  
**Status:** Specification — Ready for Implementation  

---

## Bug Summary

| ID  | Severity    | File(s)                              | Title                                                      |
|-----|-------------|--------------------------------------|------------------------------------------------------------|
| B3  | HIGH        | `src/upgrade.rs`                     | `upgrade_nixos` uses `sudo` instead of `pkexec`            |
| B4  | MEDIUM      | `src/upgrade.rs`                     | `detect_next_fedora_version` returns 1 on macro failure; hardcoded fallback stale |
| B5  | MEDIUM      | `src/ui/upgrade_page.rs`             | `connect_toggled` accumulates signal handlers per check run |
| B6  | MEDIUM      | `src/backends/nix.rs`                | `count_available` runs `nix flake update` (destructive, network) |
| B7  | MEDIUM      | `src/backends/nix.rs`                | `nix profile list` capability probe pollutes user log      |
| B8  | LOW-MEDIUM  | `src/backends/os_package_manager.rs` | `count_dnf_upgraded` always returns 0                      |
| B9  | LOW         | `src/ui/window.rs`                   | `unwrap()` on tokio runtime silently kills background threads |
| B10 | LOW         | `meson.build`                        | Binary install path mismatch                               |

**New dependencies required:** None. All fixes use existing crate dependencies (`serde_json`, `tokio`, `async_channel`, `which`, `std`).

---

## B3 — HIGH: `upgrade_nixos` uses `sudo` instead of `pkexec`

### Confirmation

**Yes, confirmed bug.**

The function `upgrade_nixos` in `src/upgrade.rs` is the only privilege-escalation site in the entire codebase that still uses `sudo`. Every other privileged call (APT, DNF, Pacman, Zypper, `nixos-rebuild`) uses `pkexec`. A GTK desktop application has no attached TTY; `sudo` will either fail immediately with "no tty present and no askpass program specified" or block indefinitely waiting for password input on a PTY that does not exist.

### Root Cause

`upgrade_nixos` predates the codebase-wide convention of using `pkexec` for privilege escalation. Two specific commands were left using `sudo`:

1. `nix-channel --update` (LegacyChannel path)
2. `nix flake update --flake /etc/nixos` (Flake path)

Both commands write to system directories owned by root (`/nix/var/nix/profiles/per-user/root` and `/etc/nixos`) and therefore require privilege escalation.

Additionally, `pkexec` resets `PATH` to a minimal set of standard directories. On NixOS, the Nix toolchain (`nix`, `nix-channel`, `nixos-rebuild`) lives under `/run/current-system/sw/bin` and `/nix/var/nix/profiles/default/bin`, which are not included in `pkexec`'s default PATH. The existing `nix.rs` backend already addresses this with a PATH-export wrapper — the same pattern must be used here.

### Exact Fix

**File:** `src/upgrade.rs`  
**Function:** `upgrade_nixos`

**Before:**
```rust
NixOsConfigType::LegacyChannel => {
    let _ = tx.send_blocking("Detected: legacy channel-based NixOS config".into());
    let _ = tx.send_blocking("Updating NixOS channel...".into());
    if !run_streaming_command("sudo", &["nix-channel", "--update"], tx) {
        return false;
    }
    let _ = tx.send_blocking("Rebuilding NixOS (switch --upgrade)...".into());
    run_streaming_command("pkexec", &["nixos-rebuild", "switch", "--upgrade"], tx)
}
NixOsConfigType::Flake => {
    let _ = tx.send_blocking("Detected: flake-based NixOS config".into());
    let _ = tx.send_blocking("Updating flake inputs in /etc/nixos...".into());
    if !run_streaming_command(
        "sudo",
        &["nix", "flake", "update", "--flake", "/etc/nixos"],
        tx,
    ) {
        return false;
    }
    let hostname = detect_hostname();
    let flake_target = format!("/etc/nixos#{}", hostname);
    let _ = tx.send_blocking(format!("Rebuilding NixOS configuration: {flake_target}"));
    run_streaming_command(
        "pkexec",
        &["nixos-rebuild", "switch", "--flake", &flake_target],
        tx,
    )
}
```

**After:**
```rust
// NixOS PATH prefix required because pkexec resets PATH, excluding Nix tooling.
const NIX_PATH_EXPORT: &str = "export PATH=/run/current-system/sw/bin:/run/wrappers/bin:\
                                /nix/var/nix/profiles/default/bin:$PATH";

NixOsConfigType::LegacyChannel => {
    let _ = tx.send_blocking("Detected: legacy channel-based NixOS config".into());
    let _ = tx.send_blocking("Updating NixOS channel...".into());
    let cmd = format!("{NIX_PATH_EXPORT} && nix-channel --update");
    if !run_streaming_command("pkexec", &["sh", "-c", &cmd], tx) {
        return false;
    }
    let _ = tx.send_blocking("Rebuilding NixOS (switch --upgrade)...".into());
    run_streaming_command("pkexec", &["nixos-rebuild", "switch", "--upgrade"], tx)
}
NixOsConfigType::Flake => {
    let _ = tx.send_blocking("Detected: flake-based NixOS config".into());
    let _ = tx.send_blocking("Updating flake inputs in /etc/nixos...".into());
    let cmd = format!("{NIX_PATH_EXPORT} && nix flake update --flake /etc/nixos");
    if !run_streaming_command("pkexec", &["sh", "-c", &cmd], tx) {
        return false;
    }
    let hostname = detect_hostname();
    let flake_target = format!("/etc/nixos#{}", hostname);
    let _ = tx.send_blocking(format!("Rebuilding NixOS configuration: {flake_target}"));
    run_streaming_command(
        "pkexec",
        &["nixos-rebuild", "switch", "--flake", &flake_target],
        tx,
    )
}
```

The `NIX_PATH_EXPORT` constant can be defined at module level (top of `upgrade.rs`) or as a local `const` inside `upgrade_nixos` — either is correct.

### Affected Files

- `src/upgrade.rs` — only

### Ripple Effects

None. `run_streaming_command` is a private helper used only within `upgrade.rs`. The change is fully contained.

---

## B4 — MEDIUM: `detect_next_fedora_version` returns 1 on macro failure; hardcoded fallback stale

### Confirmation

**Yes, confirmed bug.**

When `rpm -E %fedora` is run on a system where the `%fedora` RPM macro is not defined (e.g., in a container, on a non-Fedora RPM-based system, or if rpmdb is broken), the command outputs the literal string `%fedora` instead of a number. `.parse::<u32>()` fails on this string, `unwrap_or(0)` yields `0`, and `0 + 1 = 1`. The function then silently returns `1`, causing `dnf system-upgrade download --releasever 1 -y` to be invoked — which will fail, but only after potentially downloading stale metadata or producing a confusing error.

The hardcoded fallback of `41` (in the `else` branch when `rpm` itself cannot be executed) is also stale: Fedora 41 was released in October 2024 and reached end-of-life in May 2025. As of March 2026, Fedora 43 is the current stable release.

### Root Cause

1. The function does not distinguish between a successful `rpm` invocation that returns an undefined-macro literal versus a failed parse.
2. The fallback path (`rpm` binary not found) uses a hardcoded constant that was not updated to track the release schedule.
3. The function returns `u32`, making it impossible to signal failure to the caller; the caller unconditionally passes the returned value to `dnf`.

### Exact Fix

**File:** `src/upgrade.rs`  
**Functions:** `detect_next_fedora_version` (return type change) and `upgrade_fedora` (caller update)

**Before — `detect_next_fedora_version`:**
```rust
fn detect_next_fedora_version() -> u32 {
    let output = Command::new("rpm").args(["-E", "%fedora"]).output().ok();

    if let Some(out) = output {
        let current: u32 = String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .unwrap_or(0);
        current + 1
    } else {
        // Fallback to a reasonable version
        41
    }
}
```

**After — `detect_next_fedora_version`:**
```rust
/// Detect the next Fedora major version to upgrade to.
/// Returns `None` if the current version cannot be determined reliably.
fn detect_next_fedora_version() -> Option<u32> {
    // Primary source: rpm RPM macro (most accurate on a live Fedora system).
    if let Ok(out) = Command::new("rpm").args(["-E", "%fedora"]).output() {
        let s = String::from_utf8_lossy(&out.stdout);
        let trimmed = s.trim();
        // rpm outputs the literal string "%fedora" when the macro is undefined.
        if !trimmed.starts_with('%') {
            if let Ok(current) = trimmed.parse::<u32>() {
                return Some(current + 1);
            }
        }
    }

    // Fallback: read VERSION_ID from /etc/os-release, which is always present
    // and does not depend on rpmdb health.
    if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("VERSION_ID=") {
                let val = rest.trim_matches('"');
                if let Ok(current) = val.parse::<u32>() {
                    return Some(current + 1);
                }
            }
        }
    }

    // Both detection paths failed; signal to the caller that the upgrade
    // target cannot be determined.
    None
}
```

**Before — relevant section of `upgrade_fedora`:**
```rust
    // Detect next version
    let next_version = detect_next_fedora_version();
    let ver_str = next_version.to_string();
    if !run_streaming_command(
        "pkexec",
        &[
            "dnf",
            "system-upgrade",
            "download",
            "--releasever",
            &ver_str,
            "-y",
        ],
        tx,
    ) {
```

**After — relevant section of `upgrade_fedora`:**
```rust
    // Detect next version; fail fast if it cannot be determined rather than
    // passing an invalid --releasever argument to dnf.
    let next_version = match detect_next_fedora_version() {
        Some(v) => v,
        None => {
            let _ = tx.send_blocking(
                "Error: could not detect the current Fedora version. \
                 Cannot determine the upgrade target release. \
                 Ensure /etc/os-release is readable and VERSION_ID is set."
                    .into(),
            );
            return false;
        }
    };
    let ver_str = next_version.to_string();
    if !run_streaming_command(
        "pkexec",
        &[
            "dnf",
            "system-upgrade",
            "download",
            "--releasever",
            &ver_str,
            "-y",
        ],
        tx,
    ) {
```

### Affected Files

- `src/upgrade.rs` — `detect_next_fedora_version` signature + `upgrade_fedora` call site

### Ripple Effects

`detect_next_fedora_version` is a private function called only from `upgrade_fedora`. No other callers exist. The return type change is fully contained within this file.

---

## B5 — MEDIUM: `connect_toggled` accumulates signal handlers on each check run

### Confirmation

**Yes, confirmed bug.**

In `src/ui/upgrade_page.rs`, the `backup_check.connect_toggled(...)` call appears *inside* the async block of `check_button.connect_clicked`. The condition guarding it is:

```rust
if all_passed && upgrade_is_available {
    backup_ref.connect_toggled(move |check| { ... });
    ...
}
```

Each time the user clicks "Run Checks" and both prerequisites pass *and* an upgrade is available, a new signal handler is prepended to the `toggled` signal connection list of `backup_check`. GTK signal connections are cumulative — they do not replace existing connections. After N successful check runs, a single toggle of the checkbox fires N handlers, each independently calling `upgrade_ref.set_sensitive(...)`. While the end state is the same (the button is enabled/disabled), it is wasteful, creates N captures of the same closures alive in memory, and is conceptually incorrect — the handler should be registered exactly once when the widget is built.

### Root Cause

The signal handler was placed inside the check callback to capture the locally-computed `all_passed` boolean at the point where it is known. The correct architecture is to lift the `connect_toggled` call out of the check callback, share the `all_passed` state via `Rc<RefCell<bool>>`, and have the single persistent handler read from that shared state.

### Exact Fix

**File:** `src/ui/upgrade_page.rs`

The fix has three parts:

**Part 1 — Add shared `all_checks_passed` state** (alongside the existing `upgrade_available` declaration):

**Before:**
```rust
        // Tracks whether a distro upgrade is actually available.
        // The Start Upgrade button must not be enabled unless this is true.
        let upgrade_available: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
```

**After:**
```rust
        // Tracks whether a distro upgrade is actually available.
        // The Start Upgrade button must not be enabled unless this is true.
        let upgrade_available: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));

        // Tracks whether all prerequisite checks passed.
        // Shared with the backup_check toggled handler below.
        let all_checks_passed: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
```

**Part 2 — Connect `backup_check.connect_toggled` exactly once**, placed after the `backup_check` widget is created and before `check_button.connect_clicked`. Insert immediately after the `backup_check` widget builder block:

**Before** (no `connect_toggled` call here — it is inside `check_button.connect_clicked`):
```rust
        // Backup confirmation
        let backup_check = gtk::CheckButton::builder()
            .label("I have backed up my important data")
            .halign(gtk::Align::Center)
            .build();
        content_box.append(&backup_check);

        // Wire up check button
        let check_rows_clone = check_rows.clone();
```

**After:**
```rust
        // Backup confirmation
        let backup_check = gtk::CheckButton::builder()
            .label("I have backed up my important data")
            .halign(gtk::Align::Center)
            .build();
        content_box.append(&backup_check);

        // Connect the toggled handler exactly once here.  It reads shared
        // Rc<RefCell<>> state so it always reflects the latest check run result.
        {
            let upgrade_btn_t = upgrade_button.clone();
            let all_checks_passed_t = all_checks_passed.clone();
            let upgrade_available_t = upgrade_available.clone();
            backup_check.connect_toggled(move |check| {
                if check.is_active()
                    && *all_checks_passed_t.borrow()
                    && *upgrade_available_t.borrow()
                {
                    upgrade_btn_t.set_sensitive(true);
                } else {
                    upgrade_btn_t.set_sensitive(false);
                }
            });
        }

        // Wire up check button
        let check_rows_clone = check_rows.clone();
```

**Part 3 — Remove the `connect_toggled` call from inside the check button callback** and replace it with direct state updates + button evaluation. The clones list and the async block inside `check_button.connect_clicked` must also capture `all_checks_passed`:

**Before** (variable clone declarations before `check_button.connect_clicked`):
```rust
        let check_rows_clone = check_rows.clone();
        let check_icons_clone = check_icons.clone();
        let upgrade_btn_clone = upgrade_button.clone();
        let log_clone = log_panel.clone();
        let backup_clone = backup_check.clone();
        let distro_clone = distro_info.clone();
        let upgrade_available_clone = upgrade_available.clone();
```

**After:**
```rust
        let check_rows_clone = check_rows.clone();
        let check_icons_clone = check_icons.clone();
        let upgrade_btn_clone = upgrade_button.clone();
        let log_clone = log_panel.clone();
        let backup_clone = backup_check.clone();
        let distro_clone = distro_info.clone();
        let upgrade_available_clone = upgrade_available.clone();
        let all_checks_passed_clone = all_checks_passed.clone();
```

**Before** (tail of the `glib::spawn_future_local` block inside `check_button.connect_clicked`):
```rust
                let upgrade_is_available = *upgrade_available_ref.borrow();
                if all_passed && upgrade_is_available {
                    // Enable upgrade button only if backup is confirmed
                    let upgrade_ref2 = upgrade_ref.clone();
                    let upgrade_available_ref2 = upgrade_available_ref.clone();
                    backup_ref.connect_toggled(move |check| {
                        // Re-check availability in case the async check finished late.
                        if check.is_active() && *upgrade_available_ref2.borrow() {
                            upgrade_ref2.set_sensitive(true);
                        } else {
                            upgrade_ref2.set_sensitive(false);
                        }
                    });
                    if backup_ref.is_active() {
                        upgrade_ref.set_sensitive(true);
                    }
                }

                button_ref.set_sensitive(true);
```

**After:**
```rust
                // Update shared state so the persistent toggled handler remains correct.
                *all_checks_passed_ref.borrow_mut() = all_passed;

                // Directly evaluate the upgrade button state for the current run.
                let upgrade_is_available = *upgrade_available_ref.borrow();
                if all_passed && upgrade_is_available && backup_ref.is_active() {
                    upgrade_ref.set_sensitive(true);
                } else {
                    upgrade_ref.set_sensitive(false);
                }

                button_ref.set_sensitive(true);
```

The `all_checks_passed_ref` name is the capture of `all_checks_passed_clone` inside the `glib::spawn_future_local` async block — add it to the captures in the same way `upgrade_available_ref` is captured:

```rust
            let all_checks_passed_ref = all_checks_passed_clone.clone();
```

(Alongside the existing `let upgrade_available_ref = upgrade_available_clone.clone();` inside the async block.)

### Affected Files

- `src/ui/upgrade_page.rs` — only

### Ripple Effects

None. The `backup_check` widget and `upgrade_button` widget references are available in the same build function scope. No other files are affected.

---

## B6 — MEDIUM: Nix `count_available` has destructive network side effects

### Confirmation

**Yes, confirmed bug.**

The `count_available` implementation for flake-based NixOS in `src/backends/nix.rs`:

1. Creates a temporary directory in `/tmp`
2. Copies `/etc/nixos/flake.nix` and `/etc/nixos/flake.lock` to it
3. Runs `nix flake update` inside the temp directory

`nix flake update` is explicitly documented as an operation that fetches all flake inputs from the network and **writes updated revisions to `flake.lock`**. Even though the temp copy is deleted afterward, the command also writes to the Nix evaluation cache in the user's `~/.cache/nix/` directory. This is a write side effect that violates the read-only contract of `count_available`.

The function is invoked on window open (via `run_checks()`) and on every click of the refresh button. This means `nix flake update` — a full network fetch across all flake inputs — fires silently in the background every time the update page is shown or refreshed.

### Root Cause

The implementor confused "count updates available" with "run the update"; they used `nix flake update` as a way to force-resolve all input revisions, then counted the lines containing "Updated input". The correct approach is to inspect the existing lock state and, if a network check is desired, use a read-only metadata query.

### Exact Fix

**File:** `src/backends/nix.rs`  
**Method:** `count_available` — flake-based NixOS branch

The fix replaces the destructive temp-dir + `nix flake update` approach with a direct read of the `flake.lock` JSON file. This gives the count of flake inputs (i.e., the number of things that *can* be updated), has zero network traffic, and has zero file system write side effects. `serde_json` is already a project dependency.

**Before:**
```rust
            if is_nixos_flake() {
                // Copy flake.nix (and lock if present) to a temp dir and run
                // `nix flake update` there.  This avoids needing root and does
                // not touch /etc/nixos, while still fetching the latest input
                // revisions from the network to produce an accurate count.
                let tmpdir = std::env::temp_dir().join("up-nix-check");
                let _ = tokio::fs::remove_dir_all(&tmpdir).await;
                if tokio::fs::create_dir_all(&tmpdir).await.is_err()
                    || tokio::fs::copy("/etc/nixos/flake.nix", tmpdir.join("flake.nix"))
                        .await
                        .is_err()
                {
                    return Err("Cannot read /etc/nixos/flake.nix".to_string());
                }
                // Bring the existing lock so nix can diff against it.
                let _ = tokio::fs::copy("/etc/nixos/flake.lock", tmpdir.join("flake.lock")).await;
                let result = tokio::process::Command::new("nix")
                    .args(["flake", "update"])
                    .current_dir(&tmpdir)
                    .output()
                    .await;
                let _ = tokio::fs::remove_dir_all(&tmpdir).await;
                match result {
                    Ok(out) => {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        let count = stderr
                            .lines()
                            .filter(|l| l.contains("Updated input"))
                            .count();
                        Ok(count)
                    }
                    Err(e) => Err(format!("nix: {e}")),
                }
```

**After:**
```rust
            if is_nixos_flake() {
                // Parse /etc/nixos/flake.lock to count the number of locked flake
                // inputs.  This is a read-only, zero-network, zero-side-effect check.
                // Each locked input is a candidate for an update; the exact set of
                // inputs that have newer upstream commits can only be confirmed by
                // running "Update All".
                let lock_text =
                    tokio::fs::read_to_string("/etc/nixos/flake.lock").await.map_err(
                        |e| format!("Cannot read /etc/nixos/flake.lock: {e}"),
                    )?;
                let lock: serde_json::Value = serde_json::from_str(&lock_text)
                    .map_err(|e| format!("Cannot parse flake.lock: {e}"))?;
                // Count non-root nodes — each is a separately-lockable flake input.
                let count = lock["nodes"]
                    .as_object()
                    .map(|nodes| {
                        nodes
                            .iter()
                            .filter(|(k, v)| *k != "root" && v.get("locked").is_some())
                            .count()
                    })
                    .unwrap_or(0);
                Ok(count)
```

The `?` operator works here because the outer function returns `Result<usize, String>`. No changes to the function signature are needed.

### Affected Files

- `src/backends/nix.rs` — `count_available`, flake-NixOS branch only

### Ripple Effects

None. `serde_json` is already in `Cargo.toml`. The function signature is unchanged. The UI layer continues to call `count_available()` identically.

---

## B7 — MEDIUM: `nix profile list` pollutes user log as a capability probe

### Confirmation

**Yes, confirmed bug.**

In `src/backends/nix.rs`, the `run_update` path for non-NixOS Nix users contains:

```rust
let use_flakes = runner.run("nix", &["profile", "list"]).await.is_ok();
```

`CommandRunner::run` unconditionally emits a `$ nix profile list` entry and the full stdout/stderr of the command to the shared log channel before any actual update work begins. The user's log panel therefore shows spurious capability-probe output at the top of every update run on non-NixOS systems with Nix installed.

### Root Cause

The implementor used `runner.run()` as a convenient boolean capability probe, not realising (or not caring) that `CommandRunner` is designed for logging user-visible commands — not internal probes.

### Exact Fix

**File:** `src/backends/nix.rs`  
**Method:** `run_update` — non-NixOS branch

The correct approach is a silent filesystem check. On Nix 2.4+, profiles that use flakes have a `manifest.json` file (version 2) in the profile directory. Legacy profiles (`nix-env`) have a `manifest.nix` (a Nix expression). Checking for the existence of `manifest.json` is a reliable, instantaneous, no-subprocess, zero-log capability probe.

**Before:**
```rust
        } else {
            // Non-NixOS: update the user's nix profile.
            let use_flakes = runner.run("nix", &["profile", "list"]).await.is_ok();
            if use_flakes {
```

**After:**
```rust
        } else {
            // Non-NixOS: update the user's nix profile.
            // Detect flake-style profiles by checking for a v2 manifest JSON file.
            // This avoids using runner.run() for a capability probe which would
            // emit noise to the user-visible log channel.
            let profile_dir = std::env::var("HOME")
                .ok()
                .map(|h| std::path::PathBuf::from(h).join(".nix-profile"))
                .unwrap_or_else(|| {
                    std::path::PathBuf::from("/nix/var/nix/profiles/default")
                });
            let use_flakes = profile_dir.join("manifest.json").exists();
            if use_flakes {
```

### Affected Files

- `src/backends/nix.rs` — `run_update`, non-NixOS branch only

### Ripple Effects

None. The resulting `use_flakes` boolean drives the same branch decision as before. No new imports are required (`std::env` and `std::path::PathBuf` are already in scope or in the standard prelude).

---

## B8 — LOW-MEDIUM: DNF upgrade count always returns 0

### Confirmation

**Yes, confirmed bug.**

The current `count_dnf_upgraded` function:

```rust
fn count_dnf_upgraded(output: &str) -> usize {
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Upgraded") || trimmed.starts_with("Installed") {
            for word in trimmed.split_whitespace() {
                if let Ok(n) = word.parse::<usize>() {
                    return n;
                }
            }
        }
    }
    0
}
```

`starts_with("Upgraded")` matches the post-transaction package-list section header `"Upgraded:"` — but this line contains only a colon, no count. The function scans its tokens, finds no parseable integer, and continues. The count is on a *different* line in the Transaction Summary section, which uses `"Upgrade"` (without a trailing `d`) in DNF4 and `"Upgrading:"` in DNF5.

### Root Cause

Confusion between the post-transaction package list headers (`"Upgraded:"`, `"Installed:"`) and the Transaction Summary count lines (`"Upgrade  N Packages"` / `"Upgrading: N packages"`).

**DNF4 Transaction Summary format** (appears *before* the transaction begins):
```
Transaction Summary
============================================================
Install   1 Package
Upgrade  15 Packages
```

**DNF5 Transaction Summary format** (appears at the *end* of the transaction):
```
Transaction Summary:
 Upgrading:        15 packages
 Installing:        1 package
```

**Post-transaction package list headers** (present in both DNF4 and DNF5 — these are NOT count lines):
```
Upgraded:
  bash-5.2.26-1.fc41.x86_64
  ...
```

### Exact Fix

**File:** `src/backends/os_package_manager.rs`  
**Function:** `count_dnf_upgraded`

**Before:**
```rust
fn count_dnf_upgraded(output: &str) -> usize {
    // Look for "Upgraded:" section
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Upgraded") || trimmed.starts_with("Installed") {
            // e.g., "Upgraded  15 Packages"
            for word in trimmed.split_whitespace() {
                if let Ok(n) = word.parse::<usize>() {
                    return n;
                }
            }
        }
    }
    0
}
```

**After:**
```rust
fn count_dnf_upgraded(output: &str) -> usize {
    // DNF4 Transaction Summary line: "Upgrade  N Package(s)" or "Install  N Package(s)"
    //   — appears before the transaction; note NO trailing 'd'.
    // DNF5 Transaction Summary line: "Upgrading:  N packages" or "Installing:  N packages"
    //   — appears at the end of the transaction output.
    // The post-transaction per-package headers "Upgraded:" / "Installed:" do NOT
    // carry the count on the same line and must NOT be matched here.
    let mut total = 0usize;
    for line in output.lines() {
        let trimmed = line.trim();
        let is_dnf4_summary = (trimmed.starts_with("Upgrade ") || trimmed.starts_with("Install "))
            && !trimmed.starts_with("Upgraded")
            && !trimmed.starts_with("Installed");
        let is_dnf5_summary =
            trimmed.starts_with("Upgrading:") || trimmed.starts_with("Installing:");
        if is_dnf4_summary || is_dnf5_summary {
            for word in trimmed.split_whitespace() {
                if let Ok(n) = word.parse::<usize>() {
                    total += n;
                    break;
                }
            }
        }
    }
    total
}
```

Summing both `Upgrade` and `Install` counts and accumulating across both DNF4 and DNF5 lines gives the total net packages changed, consistent with what APT reports.

### Affected Files

- `src/backends/os_package_manager.rs` — `count_dnf_upgraded` function only

### Ripple Effects

None. `count_dnf_upgraded` is a private function called only from `DnfBackend::run_update`.

---

## B9 — LOW: `unwrap()` on tokio runtime silently kills background threads

### Confirmation

**Yes, confirmed bug.**

Two `std::thread::spawn` closures in `src/ui/window.rs` call:

```rust
let rt = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()
    .unwrap();
```

If `tokio::runtime::Builder::build()` fails (e.g., under OS resource exhaustion — too many open file descriptors, or an OS-level restriction on io_uring), the thread panics silently. The spawning GTK async future never receives a result on its `rx` channel and never completes. The UI rows remain stuck in "Checking..." or "Updating..." state with no error message and no way for the user to recover short of restarting the application.

### Root Cause

Convenience `.unwrap()` used without awareness that thread panics are not propagated to the GTK/glib main loop in any way — they are silently swallowed by Rust's default thread panic handler.

### Exact Fix

**File:** `src/ui/window.rs`

There are **two locations** to fix:

---

**Fix 1 — `run_checks` closure** (availability check per backend):

**Before:**
```rust
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    rt.block_on(async {
                        let result = backend_clone.count_available().await;
                        let _ = tx.send(result).await;
                    });
                });
```

**After:**
```rust
                std::thread::spawn(move || {
                    let rt = match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(rt) => rt,
                        Err(e) => {
                            let _ = tx.send_blocking(Err(format!(
                                "Failed to create async runtime: {e}"
                            )));
                            return;
                        }
                    };
                    rt.block_on(async {
                        let result = backend_clone.count_available().await;
                        let _ = tx.send(result).await;
                    });
                });
```

The `tx` channel carries `Result<usize, String>`, so sending `Err(...)` is type-correct. `send_blocking` is the synchronous variant provided by `async_channel::Sender` for use from non-async contexts.

---

**Fix 2 — `update_button.connect_clicked` closure** (update-all worker thread):

**Before:**
```rust
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();

                    rt.block_on(async {
                        for backend in &backends_thread {
                            let kind = backend.kind();
                            let runner = CommandRunner::new(tx_thread.clone(), kind);
                            let result = backend.run_update(&runner).await;
                            let _ = result_tx_thread.send((kind, result)).await;
                        }
                    });

                    drop(tx_thread);
                    drop(result_tx_thread);
                });
```

**After:**
```rust
                std::thread::spawn(move || {
                    let rt = match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(rt) => rt,
                        Err(e) => {
                            // Report failure for every backend so the UI rows
                            // transition to an error state instead of hanging.
                            for backend in &backends_thread {
                                let _ = result_tx_thread.send_blocking((
                                    backend.kind(),
                                    UpdateResult::Error(format!(
                                        "Failed to create async runtime: {e}"
                                    )),
                                ));
                            }
                            return;
                        }
                    };

                    rt.block_on(async {
                        for backend in &backends_thread {
                            let kind = backend.kind();
                            let runner = CommandRunner::new(tx_thread.clone(), kind);
                            let result = backend.run_update(&runner).await;
                            let _ = result_tx_thread.send((kind, result)).await;
                        }
                    });

                    drop(tx_thread);
                    drop(result_tx_thread);
                });
```

`result_tx_thread` carries `(BackendKind, UpdateResult)`, so sending `UpdateResult::Error(...)` is type-correct. The `UpdateResult` enum is already imported in `window.rs`.

### Affected Files

- `src/ui/window.rs` — two `std::thread::spawn` closures

### Ripple Effects

None. Both fixes use existing channel types and existing `UpdateResult` variants. No API changes required.

---

## B10 — LOW: `meson.build` binary install path mismatch

### Confirmation

**Yes, confirmed bug.**

The current `meson.build`:

```meson
cargo_build = custom_target('cargo-build',
  output: 'up',
  command: [cargo] + cargo_args,
  console: true,
  install: true,
  install_dir: bindir,
)
```

Meson's `custom_target` treats the `output` field as a file that the command will create **inside Meson's own build directory** (`meson.current_build_dir()`). However, Cargo writes its binary to `<source_dir>/target/debug/up` or `<source_dir>/target/release/up` — completely outside the Meson build directory. Meson will then fail to find `up` in its build directory and either raise a configuration error or silently produce a broken install. The `cargo_build` target is also not `build_always_stale`, meaning Meson may skip re-running the Cargo build because it believes the (non-existent) output is up to date.

### Root Cause

Meson's `custom_target` output tracking assumes the command writes its declared outputs into `@OUTDIR@` (the Meson target output directory). Cargo does not honour this convention — it always writes to its own `target/` directory relative to the workspace root. Without an explicit copy step, the declared `output: 'up'` is never created in the Meson build directory.

### Exact Fix

**File:** `meson.build`

The fix restructures the `custom_target` command to:
1. Run Cargo as before
2. Copy the resulting binary from Cargo's output directory into Meson's `@OUTPUT@` location

`@OUTPUT@` is a built-in Meson substitution that expands to the absolute path of the first declared output file in the Meson build directory. `build_always_stale: true` is added so Meson always re-invokes the Cargo build, allowing Cargo's own incremental compilation to determine whether a rebuild is needed.

**Before:**
```meson
if get_option('buildtype') == 'release'
  rust_target = 'release'
  cargo_args = ['build', '--release', '--manifest-path', join_paths(srcdir, 'Cargo.toml')]
else
  rust_target = 'debug'
  cargo_args = ['build', '--manifest-path', join_paths(srcdir, 'Cargo.toml')]
endif

cargo_build = custom_target('cargo-build',
  output: 'up',
  command: [cargo] + cargo_args,
  console: true,
  install: true,
  install_dir: bindir,
)
```

**After:**
```meson
if get_option('buildtype') == 'release'
  rust_target = 'release'
else
  rust_target = 'debug'
endif

cargo_build = custom_target('cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: 'up',
  command: [
    'sh', '-c',
    cargo.full_path() + ' build'
      + (rust_target == 'release' ? ' --release' : '')
      + ' --manifest-path ' + join_paths(srcdir, 'Cargo.toml')
      + ' && cp ' + join_paths(srcdir, 'target', rust_target, 'up') + ' @OUTPUT@',
  ],
  console: true,
  install: true,
  install_dir: bindir,
)
```

`cargo_args` is removed because it is no longer used (it was only referenced in the now-replaced `command` array). The `sh -c '...'` form is required because Meson executes command arrays directly without a shell, so `&&` for chaining is not available in the plain array form.

### Affected Files

- `meson.build` — only

### Ripple Effects

None. `cargo_args` is not referenced anywhere else in `meson.build`. The `install_data` calls, icon loop, and `gnome.post_install` block below the `custom_target` are unaffected.

---

## Implementation Notes

### Ordering

All 8 fixes are independent of each other and can be applied in any order or in parallel. There are no cross-fix dependencies.

### Build Verification Commands

After implementation, validate with:

```sh
cargo build
cargo clippy -- -D warnings
cargo fmt --check
cargo test
```

For the Meson fix (B10), verify separately:

```sh
meson setup builddir_test --wipe
meson compile -C builddir_test
```

### No New Dependencies

All fixes are implemented using:
- Existing standard library APIs (`std::fs`, `std::path`, `std::env`)
- Existing crate dependencies already in `Cargo.toml` (`serde_json`, `tokio`, `async_channel`)
- Meson built-in variables and string interpolation (`@OUTPUT@`, `cargo.full_path()`)

No `Cargo.toml` changes are required.
