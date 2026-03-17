# Nix Backend Fixes — Specification

## Current State Analysis

### Backend Architecture

The `Backend` trait (`src/backends/mod.rs`) defines:
- `kind()` → `BackendKind` enum variant
- `display_name()` → display string
- `description()` → subtitle text
- `icon_name()` → GTK icon name
- `run_update(&self, runner: &CommandRunner)` → performs the actual update
- `count_available(&self)` → checks how many updates are available (default returns `Ok(0)`)

All detected backends are iterated in `UpWindow::build_update_page()` (`src/ui/window.rs`). On launch (and when the refresh button is pressed), the `run_checks` closure calls `backend.count_available()` for **every** detected backend, spawning each in a separate thread with a Tokio single-threaded runtime.

### Nix Backend Current Implementation (`src/backends/nix.rs`)

The `NixBackend` implements:

1. **`count_available()`** — Three code paths:
   - **NixOS + flake**: Runs `nix flake update --dry-run /etc/nixos`, counts "Updated input" lines in stderr. Falls back to `Err("Run update to check")` if `--dry-run` isn't supported.
   - **NixOS + legacy channels**: Returns `Err("Run update to check")`.
   - **Non-NixOS (nix-env)**: Runs `nix-env -u --dry-run`, counts "upgrading" lines in stderr.

2. **`run_update()`** — Three code paths:
   - **NixOS + flake**: Runs `pkexec sh -c "nix flake update /etc/nixos && nixos-rebuild switch --flake /etc/nixos#{hostname}"`.
   - **NixOS + legacy channels**: Runs `pkexec nixos-rebuild switch --upgrade`.
   - **Non-NixOS**: Runs `nix profile upgrade .*` (flakes) or `nix-env -u` (legacy).

### Flatpak Backend Current Implementation (`src/backends/flatpak.rs`)

For reference, Flatpak's `run_update()` runs `flatpak update -y` and counts lines starting with a digit. Its `count_available()` runs `flatpak remote-ls --updates` and counts non-empty stdout lines.

---

## Problem Definitions

### Bug 1: Nix Section Does Not Check for Updates

**Symptom**: The Nix row in the UI never shows an update count on launch or when the "Check for Updates" button is pressed.

**Root Cause**: The `count_available()` implementation has the right code structure but there is a subtle issue with the `nix flake update --dry-run` approach. This method was introduced in Nix 2.19, but the argument order is wrong for newer Nix versions. Since Nix 2.20+, the correct syntax is:

```
nix flake update --flake /etc/nixos --dry-run
```

However, in older Nix (2.19), the path was a positional argument. The current code uses:

```
nix flake update --dry-run /etc/nixos
```

This may fail depending on the Nix version. But more critically, **the `count_available()` function does work** — it returns `Ok(count)` or `Err("Run update to check")`. The window code (`src/ui/window.rs` lines 252–272) handles both cases:
- `Ok(count)` → `row.set_status_available(count)`
- `Err(msg)` → `row.set_status_unknown(&msg)`

**Actual root cause identified**: On closer inspection, `count_available()` **does** get called for Nix. The NixOS flake path runs `nix flake update --dry-run /etc/nixos` which may fail with a non-zero exit code (since it runs without root but tries to access `/etc/nixos` which might require elevated permissions, or because the `--dry-run` flag is not supported). When it fails, the code returns `Err("Run update to check")` which shows as `"Run update to check"` in the UI — this is the intended fallback for NixOS.

For **non-NixOS** Nix, `count_available()` runs `nix-env -u --dry-run` which should work without privilege escalation.

**Re-evaluation**: The Nix `count_available()` has a correctness issue for the flake-based NixOS path. The `nix flake update` subcommand in newer Nix versions (≥ 2.20) changed its CLI. The positional `<flake-ref>` argument was replaced by `--flake <path>`. The `--dry-run` flag may also not exist in all versions. Additionally, even if the command syntax is correct, running `nix flake update` (even dry-run) against `/etc/nixos` may require reading the flake.nix, and if the user's Nix store daemon doesn't have access or the file permissions restrict it, it fails silently and the row shows "Run update to check".

**This behavior is actually acceptable** — NixOS flake-based updates don't have a reliable unprivileged dry-run mechanism. The fallback `"Run update to check"` message is correct UX. Bug 1 may be a misperception IF the Nix row is actually showing up. However, if the user reports it's not checking at all, the likely cause is that `count_available()` is erroring out before reaching the fallback — possibly because `tokio::process::Command::new("nix")` fails to find the `nix` binary when run from within the async context (different PATH), or the error is being silently swallowed.

**Most likely root cause**: The `count_available()` function for NixOS flake path calls `tokio::process::Command::new("nix").args(["flake", "update", "--dry-run", "/etc/nixos"])`. If this command fails with an error (non-zero exit), the function returns `Err("Run update to check")`, which is correctly handled by `set_status_unknown()`. So the check **does** appear to work, but may show a not-very-informative message.

**Revised diagnosis**: After careful examination, count_available() IS called for Nix. The check flow works. The user's complaint may be that:
1. On NixOS with flakes, it always shows "Run update to check" instead of an actual count, OR
2. The implementation incorrectly handles the `nix flake update` command syntax.

The fix should ensure a more robust check that actually provides a count when possible, and falls back gracefully.

### Bug 2: Nix Update Command `sh -c` Quoting Issue

**Symptom**: Running the NixOS flake update produces:
```
[Nix] $ pkexec sh -c nix flake update /etc/nixos && nixos-rebuild switch --flake /etc/nixos#vexos
[Nix] stderr: path '/root' does not contain a 'flake.nix', searching up
[Nix] stderr: error: could not find a flake.nix file
```

**Root Cause**: In `src/backends/nix.rs` line 56–60:
```rust
let cmd = format!(
    "nix flake update /etc/nixos && nixos-rebuild switch --flake /etc/nixos#{}",
    hostname
);
match runner.run("pkexec", &["sh", "-c", &cmd]).await {
```

The `CommandRunner::run()` method (`src/runner.rs`) passes args directly to `tokio::process::Command`:
```rust
let mut child = Command::new(program)
    .args(args)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
```

When called with `runner.run("pkexec", &["sh", "-c", &cmd])`, this invokes:
```
pkexec sh -c "nix flake update /etc/nixos && nixos-rebuild switch --flake /etc/nixos#hostname"
```

**However**, `pkexec` does NOT use a shell to interpret its arguments. It executes the command directly. So `pkexec sh -c <arg>` passes `sh` as the program and `-c` plus the command string to `sh`. The issue is that `pkexec` receives the arguments list as:
```
["sh", "-c", "nix flake update /etc/nixos && nixos-rebuild switch --flake /etc/nixos#hostname"]
```

This should actually work correctly because `Command::new("pkexec").args(["sh", "-c", &cmd])` passes the full `cmd` string as a single argument to `sh -c`. The OS-level process creation preserves argument boundaries — there's no shell re-splitting.

**Re-examining the error output**: The log shows:
```
$ pkexec sh -c nix flake update /etc/nixos && nixos-rebuild switch --flake /etc/nixos#vexos
```

This is the **display** command generated by `CommandRunner::run()`:
```rust
let display_cmd = format!("{} {}", program, args.join(" "));
```

The `args.join(" ")` joins all args with spaces, so it DISPLAYS as:
```
pkexec sh -c nix flake update /etc/nixos && nixos-rebuild switch --flake /etc/nixos#vexos
```

But the actual execution should pass the full string as one arg to `sh -c`. Let me re-examine.

Actually, looking at the Tokio `Command` API: `Command::new("pkexec").args(&["sh", "-c", &cmd])` creates a process with the argument vector `["sh", "-c", "nix flake update /etc/nixos && nixos-rebuild switch --flake /etc/nixos#hostname"]`. This IS correct at the OS level — `pkexec` receives three arguments after itself, and passes them to `sh`, which sees `-c` and the compound command string.

BUT there is a **`pkexec` specific issue**: `pkexec` does **not** pass arguments the same way as `sudo`. The `pkexec` command from polkit does not support running shell commands with `sh -c` in the way expected. `pkexec` invokes the program specified (here `sh`) and passes the remaining arguments. However, `pkexec` may alter the environment (clearing PATH, etc.) or may have restrictions.

The error message `path '/root' does not contain a 'flake.nix', searching up` suggests that `nix flake update` is running but WITHOUT the `/etc/nixos` argument — it's running from `/root` (root's home directory) and looking for a `flake.nix` there. This means `sh -c` is only receiving `nix` as its command string, and `flake`, `update`, `/etc/nixos` etc. are being treated as positional parameters `$0`, `$1`, etc.

**This IS the quoting bug.** When `pkexec` invokes `sh`, it passes `-c` and then the remaining arguments. But `sh -c` treats the FIRST non-option argument after `-c` as the command string and subsequent arguments as positional parameters (`$0`, `$1`, ...). Since `pkexec` executes:

```
sh -c "nix flake update /etc/nixos && nixos-rebuild switch --flake /etc/nixos#hostname"
```

This should work. But wait — let me look at `pkexec` behavior more carefully.

After research: `pkexec` is known to NOT group arguments. If `pkexec` receives `["sh", "-c", "nix flake update ..."]`, it should pass them through. The issue is more subtle.

Looking at the error output display again: `$ pkexec sh -c nix flake update /etc/nixos && nixos-rebuild switch --flake /etc/nixos#vexos` — this is just the display format (args joined with space). The actual execution should be fine.

**But the error says** `path '/root' does not contain a 'flake.nix'`. This means `nix flake update` is running without the `/etc/nixos` path. The Nix `flake update` subcommand's positional argument handling changed across versions:

- **Nix < 2.19**: `nix flake update` takes no positional flake-ref; it updates the flake in the current directory. To specify a path: `nix flake update --flake /etc/nixos` (but this flag didn't exist either).
- **Nix 2.19+**: The `nix flake update` subcommand was reworked. Individual inputs can be specified as positional args (not the flake path). The flake path should be set via `--flake`.

So `nix flake update /etc/nixos` is **not** the correct syntax in modern Nix. The correct command is:

```
nix flake update --flake /etc/nixos
```

Or, cd to `/etc/nixos` first:
```
cd /etc/nixos && nix flake update && nixos-rebuild switch --flake /etc/nixos#hostname
```

**This is the real root cause of Bug 2**: The `nix flake update /etc/nixos` syntax is incorrect for modern Nix versions. Nix interprets `/etc/nixos` as an input name to update, not a flake path. Since it can't find that input, it falls back to searching the current directory (`/root` for the root user under pkexec), finds no `flake.nix`, and errors.

### Bug 3: Flatpak Update "Nothing to Do" Behavior

**Symptom**: Flatpak update says "Nothing to do" but shows informational messages.

**Current code** (`src/backends/flatpak.rs`):
```rust
async fn run_update(&self, runner: &CommandRunner) -> UpdateResult {
    match runner.run("flatpak", &["update", "-y"]).await {
        Ok(output) => {
            let count = output
                .lines()
                .filter(|l| {
                    let t = l.trim();
                    t.starts_with(|c: char| c.is_ascii_digit())
                })
                .count();
            UpdateResult::Success { updated_count: count }
        }
        Err(e) => UpdateResult::Error(e),
    }
}
```

When `flatpak update -y` succeeds but nothing needs updating, `count` will be `0` and the result is `UpdateResult::Success { updated_count: 0 }`. The UI (`update_row.rs`) handles this:
```rust
pub fn set_status_success(&self, count: usize) {
    let msg = if count == 0 {
        "Up to date".to_string()
    } else {
        format!("{count} updated")
    };
    self.status_label.set_label(&msg);
    self.status_label.set_css_classes(&["success"]);
}
```

So when nothing is updated, it shows "Up to date" with a green success style. This **is correct behavior**. The informational messages from flatpak appear in the log panel which is expected. The count filter correctly identifies actual update operations (lines starting with digits from the update table) vs informational text.

**Verdict**: Bug 3 is **not a bug** — the behavior is correct.

---

## Proposed Solution Architecture

### Bug 1 Fix: Improve `count_available()` for NixOS Flake Path

The `count_available()` method for NixOS flakes uses `nix flake update --dry-run /etc/nixos` which has the same positional argument issue as the `run_update()` method. Fix the command syntax to use the correct modern Nix CLI.

**Changes to `src/backends/nix.rs`:**

In `count_available()`, the NixOS flake branch should use:
```rust
tokio::process::Command::new("nix")
    .args(["flake", "update", "--flake", "/etc/nixos", "--dry-run"])
```

Additionally, for robustness, add a fallback path that tries the legacy positional syntax if the new syntax fails, since users may be on older Nix versions.

### Bug 2 Fix: Correct `sh -c` Command for NixOS Flake Update

**Changes to `src/backends/nix.rs`:**

In `run_update()`, the NixOS flake branch currently builds:
```rust
let cmd = format!(
    "nix flake update /etc/nixos && nixos-rebuild switch --flake /etc/nixos#{}",
    hostname
);
runner.run("pkexec", &["sh", "-c", &cmd]).await
```

Fix to use correct Nix CLI syntax:
```rust
let cmd = format!(
    "cd /etc/nixos && nix flake update && nixos-rebuild switch --flake /etc/nixos#{}",
    hostname
);
runner.run("pkexec", &["sh", "-c", &cmd]).await
```

This approach:
1. Changes directory to `/etc/nixos` so `nix flake update` finds `flake.nix` in the current directory (the most universally supported syntax).
2. Keeps the `&&` chain so `nixos-rebuild` only runs if the flake update succeeds.
3. Maintains the `pkexec sh -c` pattern to run both commands under privilege escalation.

### Bug 3: No Changes Required

The Flatpak behavior is correct. When there are no updates, `flatpak update -y` exits successfully and produces informational output. The backend correctly counts 0 updated packages and the UI displays "Up to date" in green. The informational messages appear in the log panel, which is the intended behavior.

---

## Implementation Steps

### File: `src/backends/nix.rs`

#### Change 1: Fix `run_update()` NixOS flake command (Bug 2)

**Before** (lines 55–60):
```rust
let cmd = format!(
    "nix flake update /etc/nixos && nixos-rebuild switch --flake /etc/nixos#{}",
    hostname
);
match runner.run("pkexec", &["sh", "-c", &cmd]).await {
```

**After**:
```rust
let cmd = format!(
    "cd /etc/nixos && nix flake update && nixos-rebuild switch --flake /etc/nixos#{}",
    hostname
);
match runner.run("pkexec", &["sh", "-c", &cmd]).await {
```

**Rationale**: Modern `nix flake update` (Nix ≥ 2.19) does not accept a positional flake path. It updates the flake in the current directory. By `cd /etc/nixos` first, the command works across all supported Nix versions. The `&&` chain ensures atomicity — `nixos-rebuild` only runs if the flake update succeeds.

#### Change 2: Fix `count_available()` NixOS flake command (Bug 1)

**Before** (lines 103–109):
```rust
let out = tokio::process::Command::new("nix")
    .args(["flake", "update", "--dry-run", "/etc/nixos"])
    .output()
    .await
    .map_err(|e| e.to_string())?;
if out.status.success() {
    let text = String::from_utf8_lossy(&out.stderr);
    Ok(text.lines().filter(|l| l.contains("Updated input")).count())
} else {
    // Older Nix without --dry-run support.
    Err("Run update to check".to_string())
}
```

**After**:
```rust
// Try modern syntax first: nix flake update --flake <path> --dry-run
let out = tokio::process::Command::new("nix")
    .args(["flake", "update", "--flake", "/etc/nixos", "--dry-run"])
    .output()
    .await
    .map_err(|e| e.to_string())?;
if out.status.success() {
    let text = String::from_utf8_lossy(&out.stderr);
    return Ok(text.lines().filter(|l| l.contains("Updated input")).count());
}
// Fallback: run from the flake directory (works with older Nix versions)
let out = tokio::process::Command::new("nix")
    .args(["flake", "update", "--dry-run"])
    .current_dir("/etc/nixos")
    .output()
    .await
    .map_err(|e| e.to_string())?;
if out.status.success() {
    let text = String::from_utf8_lossy(&out.stderr);
    Ok(text.lines().filter(|l| l.contains("Updated input")).count())
} else {
    Err("Run update to check".to_string())
}
```

**Rationale**: First tries the `--flake` flag (modern Nix ≥ 2.20). If that fails, falls back to running `nix flake update --dry-run` with `current_dir` set to `/etc/nixos` (works with Nix 2.19 and earlier flake-enabled versions). Final fallback returns the "Run update to check" message for environments where dry-run isn't supported at all.

---

## Risks and Mitigations

### Risk 1: Nix CLI Version Fragmentation
**Risk**: Different Nix versions have different CLI syntax for `nix flake update`.
**Mitigation**: The `count_available()` fix uses a two-step fallback: `--flake` flag first, then `current_dir` approach. The `run_update()` fix uses `cd /etc/nixos && nix flake update` which works universally since `nix flake update` always looks at the current directory.

### Risk 2: `/etc/nixos` Permissions for Unprivileged Check
**Risk**: `count_available()` runs without privilege escalation. If `/etc/nixos/flake.nix` or the Nix store has restricted read permissions, the dry-run check may fail.
**Mitigation**: The existing fallback `Err("Run update to check")` handles this gracefully — the UI shows an informational message instead of a count. No crash or UX degradation.

### Risk 3: `pkexec` + `sh -c` argument passing
**Risk**: `pkexec` behavior with `sh -c` and compound shell commands may vary across polkit versions.
**Mitigation**: The `CommandRunner::run()` uses `tokio::process::Command` which passes arguments as a proper argv array. `pkexec` forwards these to `sh`, which receives `-c` and the full command string as separate argv entries. This is the standard POSIX-compliant way to invoke shell commands with privilege escalation.

### Risk 4: Hostname resolution
**Risk**: `nixos_hostname()` reads from `/proc/sys/kernel/hostname` which may differ from the NixOS configuration name.
**Mitigation**: This is a pre-existing concern not introduced by this change. The fallback `"nixos"` is already in place. No change needed for this fix.

---

## Files to Modify

| File | Changes |
|------|---------|
| `src/backends/nix.rs` | Fix `run_update()` flake command syntax; Fix `count_available()` flake command syntax with fallback |

## No Changes Required

| File | Reason |
|------|--------|
| `src/backends/flatpak.rs` | Bug 3 is not a bug — behavior is correct |
| `src/backends/mod.rs` | No trait changes needed |
| `src/runner.rs` | Command runner works correctly; the issue is in the command arguments |
| `src/ui/window.rs` | Update check flow works correctly for all backends |
| `src/ui/update_row.rs` | All status display methods are correct |
