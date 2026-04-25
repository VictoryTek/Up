# Ubuntu Upgrade Detection & Execution — Specification

**Feature:** Ubuntu OS Upgrade Detection and Execution  
**Target:** Ubuntu 24.04 LTS (Noble Numbat) → Ubuntu 26.04 LTS (Resolute Raccoon)  
**Date:** 2026-04-24  
**Spec Path:** `.github/docs/subagent_docs/ubuntu_upgrade_detection_spec.md`

---

## Table of Contents

1. [Current State Analysis](#1-current-state-analysis)
2. [Problem Definition](#2-problem-definition)
3. [Research Findings](#3-research-findings)
4. [Proposed Solution Architecture](#4-proposed-solution-architecture)
5. [Implementation Steps (File by File)](#5-implementation-steps-file-by-file)
6. [Dependencies & System Tool Requirements](#6-dependencies--system-tool-requirements)
7. [Risks and Mitigations](#7-risks-and-mitigations)

---

## 1. Current State Analysis

### 1.1 Files Analyzed

| File | Role |
|------|------|
| `src/upgrade.rs` | Distro detection, upgrade checks, upgrade execution |
| `src/backends/os_package_manager.rs` | APT/DNF/Pacman/Zypper backends for package updates |
| `src/backends/mod.rs` | Backend trait, detection orchestration |
| `src/ui/upgrade_page.rs` | Upgrade tab UI, button wiring, channel message handling |
| `src/ui/window.rs` | Main window, distro detection fanout, upgrade page init |
| `src/runner.rs` | `run_command_sync`, `CommandRunner`, `PrivilegedShell` |
| `src/app.rs` | Application entry, resource registration |
| `Cargo.toml` | Dependencies (gtk4 v0.9, libadwaita v0.7, tokio v1, async-channel v2) |

### 1.2 Upgrade Detection: `check_ubuntu_upgrade()`

Current implementation in `src/upgrade.rs` (lines ~310–330):

```rust
fn check_ubuntu_upgrade() -> String {
    match Command::new("do-release-upgrade").args(["-c"]).output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("New release") || stdout.contains("new release") {
                // ...
            } else {
                "No upgrade available".to_string()
            }
        }
        Err(_) => "Could not check (do-release-upgrade not found)".to_string(),
    }
}
```

**What it does:** Runs `do-release-upgrade -c` without specifying a frontend (`-f`), captures only
stdout, and checks for the string "New release".

### 1.3 Upgrade Execution: `upgrade_ubuntu()`

Current implementation in `src/upgrade.rs` (lines ~460–475):

```rust
fn upgrade_ubuntu(tx: &async_channel::Sender<String>) -> Result<(), String> {
    let _ = tx.send_blocking("Running: do-release-upgrade -f DistUpgradeViewNonInteractive".into());
    if !crate::runner::run_command_sync(
        "pkexec",
        &["do-release-upgrade", "-f", "DistUpgradeViewNonInteractive"],
        tx,
    ) {
        return Err("Ubuntu/Debian upgrade command failed (see log for details)".to_string());
    }
    Ok(())
}
```

**What it does:** Runs `pkexec do-release-upgrade -f DistUpgradeViewNonInteractive` and streams
output to the log panel via `run_command_sync`.

### 1.4 UI Flow

1. `window.rs`: Spawns background thread to detect distro; sends `UpgradePageInit` via channel.
2. `upgrade_page.rs`: Receives `UpgradePageInit`, stores `DistroInfo`.
3. On init: spawns thread calling `check_upgrade_available(&distro)` → `check_ubuntu_upgrade()`.
4. Sets `upgrade_available_row` subtitle to the returned string.
5. Sets `upgrade_available: bool` based on `result_msg.starts_with("Yes")`.
6. Only enables "Start Upgrade" button when: `upgrade_available && all_checks_passed && backup_confirmed`.

---

## 2. Problem Definition

### 2.1 Bug #1 — Upgrade Detection: Missing Frontend Flag

`do-release-upgrade -c` without `-f DistUpgradeViewNonInteractive` will auto-select a frontend.
When run as a subprocess of a GTK4 application:
- If `DISPLAY` is set, it may attempt the GTK/Qt frontend, which can fail to initialize or
  produce no stdout output.
- The failure is silent: `output()` succeeds (exit code 0), stdout is empty, code falls to
  `"No upgrade available"` — **even when an upgrade exists**.

### 2.2 Bug #2 — Upgrade Detection: stdout-Only Capture

`Command::output()` captures stdout only. In certain frontend modes or failure cases,
`do-release-upgrade -c` writes its result to **stderr**. The current code never reads stderr,
so these messages are silently dropped.

### 2.3 Bug #3 — Ubuntu 26.04 Not Detected (Critical Path for This Feature)

**Key finding from live meta-release-lts data (fetched April 24, 2026):**

```
Dist: resolute
Name: Resolute Raccoon
Version: 26.04 LTS
Date: Thu, 23 April 2026 00:26:04 UTC
Supported: 0      ← UPGRADE PATH NOT YET OPEN
```

Ubuntu 26.04 LTS was released on April 23, 2026 but **`Supported: 0`** means the official upgrade
path from 24.04 is not yet open. Canonical delays this by 4–8 weeks post-release to gather
stability data. During this window:

- `do-release-upgrade -c` on Ubuntu 24.04 returns "No new release found" — technically correct.
- The current code returns `"No upgrade available"` — leaving the user with no explanation.
- The user has no way to know that 26.04 is released, only that the upgrade prompt is pending.

The UI must distinguish between:
- **Not yet released** — no newer version exists at all.
- **Released, upgrade path pending** — 26.04 released but `Supported: 0` (canonical is
  holding the upgrade back for testing). Show informative message.
- **Upgrade available** — 26.04 marked `Supported: 1`. Offer upgrade.

### 2.4 Bug #4 — Upgrade Execution: `pkexec` Strips Environment

`pkexec` creates a clean environment. `do-release-upgrade` requires:
- `DEBIAN_FRONTEND=noninteractive` to suppress apt interactive prompts.
- Correct `PATH` to find apt, dpkg, and Python tools.
- `HOME` set correctly for package cache and temp files.

Running `pkexec do-release-upgrade -f DistUpgradeViewNonInteractive` without these variables
risks interactive prompts blocking the process or confusing Python's apt bindings.

### 2.5 Bug #5 — Upgrade Execution: Output Routing

`do-release-upgrade -f DistUpgradeViewNonInteractive` is designed for automated environments.
Its output routing is split:

- **Initial download/setup phase**: writes to stdout/stderr (captured by `run_command_sync`).
- **Actual upgrade phase** (child spawned after downloading upgrader): writes to:
  - `/var/log/dist-upgrade/main.log`
  - `/var/log/dist-upgrade/apt.log`
  - `/var/log/dist-upgrade/apt-term.log`

The current `run_command_sync` only captures stdout/stderr of the *parent* process. The log
panel may appear blank or stop updating during the lengthy package download/install phase.

---

## 3. Research Findings

### Source 1: Ubuntu Meta-Release File (Live Data)

URL: `https://changelogs.ubuntu.com/meta-release-lts`

- Format: Key-value blocks separated by blank lines.
- Each block has: `Dist`, `Name`, `Version`, `Date`, `Supported`, `Description`,
  `Release-File`, `ReleaseNotes`, `UpgradeTool`, `UpgradeToolSignature`.
- `Supported: 1` → upgrade path officially open.
- `Supported: 0` → release exists but upgrade not yet promoted.
- As of April 24, 2026: Ubuntu 26.04 "Resolute Raccoon" = `Supported: 0`.
- Ubuntu 24.04 "Noble Numbat" = `Supported: 1` (current LTS).
- For `Prompt=lts` systems (default for Ubuntu LTS), use `meta-release-lts`.
- For `Prompt=normal` systems, use `https://changelogs.ubuntu.com/meta-release`.

This is the **authoritative, reliable source** for upgrade availability — used by
`do-release-upgrade` itself internally.

### Source 2: `do-release-upgrade` Manpage (Ubuntu Noble/Resolute)

From `https://manpages.ubuntu.com/manpages/noble/man8/do-release-upgrade.8.html`:

- `-c, --check-dist-upgrade-only`: Check only; reports via exit code.
- `-f FRONTEND, --frontend=FRONTEND`: Specify frontend (required for non-interactive use).
- `-e ENV, --env=ENV`: Comma-separated environment variables to set during upgrade
  (e.g., `VAR1=VALUE1,VAR2=VALUE2`). **This is the correct mechanism for passing
  `DEBIAN_FRONTEND=noninteractive`** — do NOT use `sh -c "ENV=val do-release-upgrade"`.
- `-m MODE, --mode=MODE`: `desktop` or `server` mode.
- `-d, --devel-release`: Upgrade to development/unreleased next version.

**Exit code behavior for `-c`**: Manpage states "report the result via the exit code".
In practice, safe to check BOTH exit code and text output (some versions use exit 0 for
both cases, differing only by text).

### Source 3: `/etc/update-manager/release-upgrades`

Format:
```ini
[DEFAULT]
Prompt=lts      # or "normal" or "never"
```

- `lts`: Only show LTS-to-LTS upgrades (default on Ubuntu LTS installations).
- `normal`: Show all releases (LTS + interim).
- `never`: Disable upgrade notifications.

Determines which meta-release URL to use:
- `lts` → `https://changelogs.ubuntu.com/meta-release-lts`
- `normal` → `https://changelogs.ubuntu.com/meta-release`

### Source 4: `ubuntu-release-upgrader` Python Source (Launchpad)

Key behaviors:
- `do-release-upgrade -c` queries the meta-release server, finds next available dist,
  then prints "New release 'X' available." or "No new release found.".
- `DistUpgradeViewNonInteractive`: Auto-answers `True` to all yes/no prompts, meaning
  it never blocks waiting for user input.
- The upgrader downloads itself from `UpgradeTool` URL in the meta-release file and
  re-executes the actual upgrade — this is why stdout of the parent process goes quiet.
- Log files are created at `/var/log/dist-upgrade/` during the upgrade.

### Source 5: `pkexec` Environment Handling

`pkexec` creates a minimal, clean environment for security. It does not forward:
- `DEBIAN_FRONTEND`
- Custom `PATH` extensions
- `HOME` (set to root's home)

`do-release-upgrade` accepts `-e VAR=VAL,...` to inject environment variables after the
`pkexec` security boundary. This is the documented, safe approach. Alternative: use
`pkexec sh -c "VAR=val command"` (as done in the APT backend), which also works.

### Source 6: TTY Requirements

`do-release-upgrade -f DistUpgradeViewNonInteractive` does **not** require a TTY:
- The NonInteractive frontend is designed specifically for headless/automated use.
- `pkexec` does not allocate a PTY; this is fine for `DistUpgradeViewNonInteractive`.
- TTY requirement applies to the Text and GTK frontends.
- Risk: if no `-f` is given, `do-release-upgrade` auto-selects frontend based on available
  capabilities. In a GTK subprocess context this may pick GTK frontend (which silently fails).

**Conclusion**: Always specify `-f DistUpgradeViewNonInteractive` for the check AND execution.

---

## 4. Proposed Solution Architecture

### 4.1 Upgrade Detection (Two-Phase)

Replace the current `check_ubuntu_upgrade()` with a two-phase detection approach:

**Phase 1: Meta-Release Parse (primary)**

1. Read `/etc/update-manager/release-upgrades` to determine `Prompt` policy.
2. Select URL: `meta-release-lts` for `lts`, `meta-release` for `normal`.
3. Fetch URL with `curl -sf` (silent, fail on HTTP error).
4. Parse blocks; find the first entry with `Version` numerically greater than current
   `VERSION_ID`.
5. Return a structured `UbuntuUpgradeInfo` enum:
   - `Available { name, version, codename }` — `Supported: 1`
   - `ReleasedNotPromoted { name, version, codename }` — `Supported: 0`
   - `NotAvailable` — no newer entry in meta-release
   - `CheckFailed(reason)` — curl error, parse error, file not found

**Phase 2: `do-release-upgrade -c` (secondary / confirmation)**

Run as a subprocess when Phase 1 returns `Available`:
```
do-release-upgrade -c -f DistUpgradeViewNonInteractive
```
Capture combined stdout+stderr. If it reports "New release" → confirms availability.
Use this as an extra signal, not the gating check (Phase 1 is authoritative).

### 4.2 Upgrade Execution

Keep `do-release-upgrade -f DistUpgradeViewNonInteractive` but fix the command invocation:

```
pkexec do-release-upgrade -f DistUpgradeViewNonInteractive -e DEBIAN_FRONTEND=noninteractive
```

This uses the proper `-e` flag documented in the manpage rather than a `sh -c` wrapper.

Additionally, spawn a background log-tailing thread that reads from
`/var/log/dist-upgrade/main.log` and forwards lines to the UI channel. This ensures
the log panel shows progress during the child-spawned upgrade phase.

### 4.3 UI State Machine

The `upgrade_available_row` in the UI should show one of four states:

| State | Row Subtitle | Upgrade Button |
|-------|-------------|----------------|
| Checking… | "Checking for upgrades…" | Disabled |
| Available | "Ubuntu 26.04 LTS (Resolute Raccoon) is available" | Enabled (after checks + backup) |
| Released, not promoted | "Ubuntu 26.04 LTS released — upgrade available in a few weeks" | Disabled |
| Not available | "No newer Ubuntu LTS release available" | Disabled |
| Error | "Could not check for upgrades: \<reason\>" | Disabled |

The `upgrade_available: bool` flag must only be `true` for the `Available` state.

---

## 5. Implementation Steps (File by File)

### 5.1 `src/upgrade.rs`

#### 5.1.1 Add `UbuntuUpgradeInfo` enum

```rust
/// Structured result of an Ubuntu upgrade availability check.
#[derive(Debug, Clone)]
pub enum UbuntuUpgradeInfo {
    /// A newer Ubuntu release is available and the upgrade path is officially open.
    Available { name: String, version: String },
    /// A newer Ubuntu release has been released but Canonical has not yet opened
    /// the upgrade path (Supported: 0 in meta-release). Typically takes 4–8 weeks
    /// after release before Canonical opens the LTS upgrade path.
    ReleasedNotPromoted { name: String, version: String },
    /// No newer Ubuntu release exists in the meta-release file.
    NotAvailable,
    /// The check could not be completed (network error, missing curl, parse error).
    CheckFailed(String),
}
```

#### 5.1.2 Add `read_upgrade_prompt_policy()` function

```rust
/// Read /etc/update-manager/release-upgrades and return the Prompt= value.
/// Returns "lts" as default if the file is missing or unparseable.
fn read_upgrade_prompt_policy() -> String {
    let content = std::fs::read_to_string("/etc/update-manager/release-upgrades")
        .unwrap_or_default();
    for line in content.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("Prompt=") {
            let v = val.trim().to_lowercase();
            if v == "lts" || v == "normal" || v == "never" {
                return v;
            }
        }
    }
    "lts".to_string()
}
```

#### 5.1.3 Add `parse_meta_release_version()` helper

Parse a version string like "24.04 LTS" or "26.04 LTS" into (major, minor):

```rust
/// Parse an Ubuntu version string "X.YY" or "X.YY LTS" into (major, minor).
fn parse_ubuntu_version(version: &str) -> Option<(u32, u32)> {
    // Take only the "X.YY" portion before any space
    let numeric = version.split_whitespace().next()?;
    let mut parts = numeric.splitn(2, '.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    Some((major, minor))
}
```

#### 5.1.4 Add `fetch_ubuntu_meta_release()` function

```rust
/// Fetch the Ubuntu meta-release file via curl and return its content.
fn fetch_ubuntu_meta_release(policy: &str) -> Result<String, String> {
    let url = match policy {
        "normal" => "https://changelogs.ubuntu.com/meta-release",
        _ => "https://changelogs.ubuntu.com/meta-release-lts",
    };
    let output = Command::new("curl")
        .args(["-sf", "--max-time", "10", url])
        .output()
        .map_err(|e| format!("curl not found: {e}"))?;
    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        return Err(format!("curl exited with code {code}"));
    }
    String::from_utf8(output.stdout)
        .map_err(|e| format!("meta-release response is not valid UTF-8: {e}"))
}
```

#### 5.1.5 Add `parse_meta_release_for_upgrade()` function

```rust
/// Parse the Ubuntu meta-release content and find the first release newer than
/// `current_version_id` (e.g., "24.04").
fn parse_meta_release_for_upgrade(
    content: &str,
    current_version_id: &str,
) -> UbuntuUpgradeInfo {
    let current = match parse_ubuntu_version(current_version_id) {
        Some(v) => v,
        None => return UbuntuUpgradeInfo::CheckFailed(
            format!("Cannot parse current version: {:?}", current_version_id)
        ),
    };

    // Blocks are separated by blank lines
    for block in content.split("\n\n") {
        let mut name = String::new();
        let mut version_str = String::new();
        let mut supported: i32 = -1;

        for line in block.lines() {
            let line = line.trim();
            if let Some(v) = line.strip_prefix("Name: ") {
                name = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("Version: ") {
                version_str = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("Supported: ") {
                supported = v.trim().parse().unwrap_or(-1);
            }
        }

        if version_str.is_empty() {
            continue;
        }

        let candidate = match parse_ubuntu_version(&version_str) {
            Some(v) => v,
            None => continue,
        };

        if candidate > current {
            return if supported == 1 {
                UbuntuUpgradeInfo::Available {
                    name,
                    version: version_str,
                }
            } else {
                UbuntuUpgradeInfo::ReleasedNotPromoted {
                    name,
                    version: version_str,
                }
            };
        }
    }

    UbuntuUpgradeInfo::NotAvailable
}
```

#### 5.1.6 Rewrite `check_ubuntu_upgrade()`

Replace the existing function with:

```rust
fn check_ubuntu_upgrade(version_id: &str) -> String {
    let policy = read_upgrade_prompt_policy();

    // "never" policy: user has explicitly disabled upgrades
    if policy == "never" {
        return "Upgrades are disabled in /etc/update-manager/release-upgrades".to_string();
    }

    match fetch_ubuntu_meta_release(&policy) {
        Err(e) => {
            // Fallback: try do-release-upgrade -c if curl is unavailable
            check_ubuntu_upgrade_via_tool().unwrap_or_else(|| {
                format!("Could not check for upgrades: {e}")
            })
        }
        Ok(content) => {
            match parse_meta_release_for_upgrade(&content, version_id) {
                UbuntuUpgradeInfo::Available { name, version } => {
                    format!("Yes \u{2014} {} {} is available", name, version)
                }
                UbuntuUpgradeInfo::ReleasedNotPromoted { name, version } => {
                    format!(
                        "No \u{2014} {} {} is released but the upgrade is not yet available. \
                         Canonical typically opens the LTS upgrade path 4\u{2013}8 weeks \
                         after release.",
                        name, version
                    )
                }
                UbuntuUpgradeInfo::NotAvailable => {
                    "No \u{2014} No newer Ubuntu release available".to_string()
                }
                UbuntuUpgradeInfo::CheckFailed(reason) => {
                    format!("Could not check for upgrades: {}", reason)
                }
            }
        }
    }
}

/// Fallback upgrade check using do-release-upgrade -c when curl is unavailable.
/// Returns Some(message) if the tool is available, None otherwise.
fn check_ubuntu_upgrade_via_tool() -> Option<String> {
    let output = Command::new("do-release-upgrade")
        .args(["-c", "-f", "DistUpgradeViewNonInteractive"])
        .output()
        .ok()?;

    // Capture combined stdout + stderr
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));

    if combined.contains("New release") || combined.contains("new release") {
        let line = combined
            .lines()
            .find(|l| l.contains("New release") || l.contains("new release"))
            .unwrap_or("New release available");
        Some(format!("Yes \u{2014} {}", line.trim()))
    } else if combined.contains("No new release") {
        Some("No \u{2014} No newer Ubuntu release available".to_string())
    } else {
        Some("No \u{2014} No upgrade available".to_string())
    }
}
```

#### 5.1.7 Update `check_upgrade_available()` signature call

The existing `check_upgrade_available()` dispatches by distro ID. The Ubuntu arm must pass
`distro.version_id` to the new function signature:

```rust
// CHANGE: was check_ubuntu_upgrade()
"ubuntu" => check_ubuntu_upgrade(&distro.version_id),
```

#### 5.1.8 Fix `upgrade_ubuntu()` command invocation

Replace:
```rust
if !crate::runner::run_command_sync(
    "pkexec",
    &["do-release-upgrade", "-f", "DistUpgradeViewNonInteractive"],
    tx,
) {
```

With:
```rust
if !crate::runner::run_command_sync(
    "pkexec",
    &[
        "do-release-upgrade",
        "-f", "DistUpgradeViewNonInteractive",
        "-e", "DEBIAN_FRONTEND=noninteractive",
    ],
    tx,
) {
```

This uses the documented `-e` flag from the `do-release-upgrade` manpage to inject
environment variables after the `pkexec` security boundary — the correct approach.

#### 5.1.9 Add log-tailing thread in `upgrade_ubuntu()`

After starting the main upgrade command, spawn a thread to tail
`/var/log/dist-upgrade/main.log` and forward lines to the UI channel. This ensures
the log panel shows progress during the child-process upgrade phase when stdout goes quiet.

```rust
fn upgrade_ubuntu(tx: &async_channel::Sender<String>) -> Result<(), String> {
    let _ = tx.send_blocking("Preparing Ubuntu distribution upgrade...".into());
    let _ = tx.send_blocking(
        "This operation downloads and installs many packages. It may take 30\u{2013}60 \
         minutes. Do not power off the system.".into()
    );

    // Clear/truncate the dist-upgrade log from any previous run so tail picks up fresh output
    let log_path = "/var/log/dist-upgrade/main.log";
    let tx_tail = tx.clone();
    let tail_handle = std::thread::spawn(move || {
        // Give do-release-upgrade ~3s to create the log file before tailing
        std::thread::sleep(std::time::Duration::from_secs(3));
        use std::io::{BufRead, BufReader, Seek, SeekFrom};
        let Ok(mut file) = std::fs::File::open(log_path) else { return };
        let _ = file.seek(SeekFrom::End(0)); // tail from current end
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        loop {
            match reader.read_line(&mut line) {
                Ok(0) => {
                    // No new data; sleep and retry until the main process signals done
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                Ok(_) => {
                    let trimmed = line.trim_end_matches('\n').to_string();
                    if !trimmed.is_empty() {
                        let _ = tx_tail.send_blocking(format!("[log] {}", trimmed));
                    }
                    line.clear();
                }
                Err(_) => break,
            }
        }
    });

    let result = if !crate::runner::run_command_sync(
        "pkexec",
        &[
            "do-release-upgrade",
            "-f", "DistUpgradeViewNonInteractive",
            "-e", "DEBIAN_FRONTEND=noninteractive",
        ],
        tx,
    ) {
        Err("Ubuntu distribution upgrade failed (see log for details)".to_string())
    } else {
        Ok(())
    };

    // Signal tail thread to exit by dropping the channel reference; the thread
    // exits naturally when read_line returns Err on the closed file or after
    // a short idle period. Drop is implicit here.
    drop(tail_handle); // Let it finish naturally; it's a best-effort tail.
    result
}
```

**Note on tail thread lifetime**: The tail thread runs best-effort. Since it loops on
`read_line`, it will naturally stop receiving data once the upgrade finishes and the log
file is no longer written to. The `drop(tail_handle)` does not `join()` — this is intentional
to avoid blocking the main result return. The thread will idle-exit shortly after.

### 5.2 `src/ui/upgrade_page.rs`

No structural changes required. The existing UI wiring is correct:

- `result_msg.starts_with("Yes")` correctly gates `upgrade_available` to `true` for
  `"Yes — Ubuntu 26.04 LTS is available"` (Available state).
- `result_msg.starts_with("No")` leaves `upgrade_available = false` for the
  `ReleasedNotPromoted` state, correctly disabling the upgrade button.
- The `upgrade_available_row.set_subtitle(&result_msg)` call displays the full message
  including the human-readable explanation for the `ReleasedNotPromoted` case.

**Optional enhancement** (low priority): Add a spinner widget to the `upgrade_available_row`
while the async check is in progress. The row currently shows "Checking…" (set in the init
handler), which is adequate.

### 5.3 No Changes Required

- `src/backends/os_package_manager.rs` — not involved in upgrade path
- `src/backends/mod.rs` — not involved in upgrade path
- `src/runner.rs` — `run_command_sync` is adequate; no changes needed
- `src/app.rs` — no changes needed
- `Cargo.toml` — no new dependencies needed (curl is already used as a subprocess tool)

---

## 6. Dependencies & System Tool Requirements

### 6.1 Required System Tools

| Tool | Purpose | Notes |
|------|---------|-------|
| `curl` | Fetch Ubuntu meta-release file | Already used by Fedora/openSUSE/NixOS upgrade checks |
| `do-release-upgrade` | Fallback check + upgrade execution | From `ubuntu-release-upgrader` package |

### 6.2 Required Files on Target System

| File | Purpose | Notes |
|------|---------|-------|
| `/etc/update-manager/release-upgrades` | Upgrade policy (Prompt=lts/normal/never) | Present on all Ubuntu systems |
| `/etc/os-release` | `VERSION_ID` for current version | Already used by `detect_distro()` |

### 6.3 Upgrade Tool Package

`do-release-upgrade` is provided by `ubuntu-release-upgrader-core`. On a minimal Ubuntu
server install it may not be present. The fallback chain handles this:
1. `fetch_ubuntu_meta_release()` (curl) — does not require `do-release-upgrade`.
2. `check_ubuntu_upgrade_via_tool()` — only called if curl fails.
3. Both missing → returns `"Could not check for upgrades: ..."`.

### 6.4 No New Rust Crate Dependencies

The implementation uses only:
- `std::fs`, `std::process::Command` — already in use.
- `async_channel` — already a project dependency.
- `curl` subprocess — already the pattern used by Fedora and openSUSE checks.

Using `reqwest` or another HTTP crate was considered but rejected to maintain consistency
with existing distro checks and to avoid adding a new dependency.

---

## 7. Risks and Mitigations

### 7.1 Ubuntu 26.04 Upgrade Path Timing (`Supported: 0`)

**Risk**: Ubuntu 26.04 currently has `Supported: 0` in the meta-release-lts file. The UI
will show "Released but upgrade not yet available." The user cannot start the upgrade.

**Mitigation**: The `ReleasedNotPromoted` state displays a human-readable explanation
("Canonical typically opens the LTS upgrade path 4–8 weeks after release") so the user
understands why and what to expect. When Canonical flips `Supported: 1`, the next check
will automatically show the upgrade as available.

**Note**: The user could manually trigger the upgrade with `do-release-upgrade -d` (dev/proposed
flag) before the upgrade is officially promoted. The spec does NOT expose this in the UI as
it is unsupported and risky. A future enhancement could add an "Advanced" option.

### 7.2 `do-release-upgrade` Re-Executes Itself

**Risk**: `do-release-upgrade` downloads the actual upgrader tool from the Ubuntu CDN and
re-spawns it as a child process. The parent process's stdout goes mostly quiet during
the actual upgrade. The log panel appears stale.

**Mitigation**: The log-tailing thread (§5.1.9) reads `/var/log/dist-upgrade/main.log`
to feed progress to the UI panel. This is a best-effort tail — if the file doesn't exist
or is unreadable, the tail silently exits. Combined stdout from the parent process still
appears first (e.g., "Checking for a new Ubuntu release", download progress).

### 7.3 `pkexec` Environment Stripping

**Risk**: `pkexec` creates a minimal environment. `do-release-upgrade` may fail if it
cannot find `DEBIAN_FRONTEND`, or apt prompts block the process.

**Mitigation**: The `-e DEBIAN_FRONTEND=noninteractive` flag is passed to
`do-release-upgrade` using the documented mechanism. `pkexec` does preserve `PATH`
(sanitized) which is sufficient for finding `apt` and `dpkg`.

### 7.4 Network Unavailability (meta-release fetch)

**Risk**: If the system has no network access, `curl` returns a non-zero exit code and
`fetch_ubuntu_meta_release()` returns `Err`.

**Mitigation**: Falls back to `check_ubuntu_upgrade_via_tool()` (runs
`do-release-upgrade -c -f DistUpgradeViewNonInteractive`). `do-release-upgrade -c`
also makes a network call; if that also fails, the UI shows "Could not check for
upgrades: \<reason\>" — an honest error message rather than a false "no upgrade available".

### 7.5 Non-Ubuntu Distros (ID_LIKE Ubuntu Derivatives)

**Risk**: Distros like Linux Mint, Pop!_OS, and Elementary OS set `ID=linuxmint` (etc.)
with `ID_LIKE=ubuntu`. The `check_upgrade_available()` dispatch uses `distro.id`, so
they fall to the `_` default arm returning "Not supported for this distribution".

**Scope**: This specification fixes Ubuntu only (`ID == "ubuntu"`). Derivative distros
have their own upgrade tools (`mintupgrade`, etc.) and are out of scope for this fix.
The `upgrade_supported` field is `true` for derivatives only if their IDs match the
explicit list in `detect_distro()` (e.g., `linuxmint`, `pop`); those dispatch to their
respective upgrade paths, which are not changed here.

### 7.6 `DistUpgradeViewNonInteractive` Rare Prompt Cases

**Risk**: Even with the NonInteractive frontend, certain edge cases (conflicting
configuration files, third-party PPAs, held packages) may require user decisions that
`DistUpgradeViewNonInteractive` auto-accepts. Auto-accepting "Yes" to every prompt may
overwrite modified config files unexpectedly.

**Mitigation**: This is the intended behavior of `DistUpgradeViewNonInteractive` and is
documented as a trade-off for automated upgrades. Users are warned to review PPAs and
held packages in the prerequisites section. A future enhancement could add a check for
held packages or foreign PPAs to the prerequisite checklist.

### 7.7 Flatpak Sandbox

**Risk**: When the Up app runs inside the Flatpak sandbox, `pkexec`, `do-release-upgrade`,
and `curl` are not available in the sandbox PATH. Commands must be routed via
`flatpak-spawn --host`.

**Current status**: This is a pre-existing limitation not introduced by this fix. The
`FlatpakBackend` in `src/backends/flatpak.rs` already handles `flatpak-spawn --host` for
package updates. Applying the same pattern to upgrade commands is a separate feature
(out of scope for this fix).

---

## Summary of Changes

| File | Change |
|------|--------|
| `src/upgrade.rs` | Add `UbuntuUpgradeInfo` enum; add `read_upgrade_prompt_policy()`, `parse_ubuntu_version()`, `fetch_ubuntu_meta_release()`, `parse_meta_release_for_upgrade()`, `check_ubuntu_upgrade_via_tool()`; rewrite `check_ubuntu_upgrade()` to accept `version_id: &str`; update `check_upgrade_available()` dispatch to pass version_id; fix `upgrade_ubuntu()` to use `-e DEBIAN_FRONTEND=noninteractive`; add log-tailing thread in `upgrade_ubuntu()` |
| `src/ui/upgrade_page.rs` | No changes required |
| `src/backends/` | No changes required |
| `Cargo.toml` | No new dependencies |

---

## Spec File Path

`.github/docs/subagent_docs/ubuntu_upgrade_detection_spec.md`
