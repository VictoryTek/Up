# Specification: Fedora Distribution Upgrade Fix

**Feature name:** `fedora_upgrade_fix`  
**Date:** 2026-05-03  
**Status:** Draft  
**Priority:** Critical  

---

## 1. Current State Analysis

### File: `src/upgrade.rs` — `upgrade_fedora()`

The current implementation of `upgrade_fedora()` performs three steps:

1. **Plugin install**: `pkexec dnf install -y dnf-plugin-system-upgrade`
2. **Package download**: `pkexec dnf system-upgrade download --releasever <N+1> -y`
3. **Reboot trigger**: `pkexec dnf system-upgrade reboot` via `run_command_sync()`

### File: `src/runner.rs` — `run_command_sync()`

- Spawns the given process, drains stdout and stderr concurrently to the log channel.
- Waits for the process to exit via `child.wait()`.
- Returns `true` if `exit_status.success()`, `false` otherwise.
- This is a **blocking, wait-for-exit** function — it does not return until the spawned process terminates.

### File: `src/ui/upgrade_page.rs` — upgrade button handler

After `upgrade::execute_upgrade()` returns `Ok(())`, the UI calls:
```rust
crate::ui::reboot_dialog::show_reboot_dialog(&button_ref3);
```
This shows a dialog prompting the user to reboot manually.

### File: `src/reboot.rs` — `reboot()`

Uses `Command::new("systemctl").arg("reboot").spawn()` — fire-and-forget (no `pkexec`, runs as the current user).

### DNF4 vs DNF5 context

| Fedora Version | Package Manager | `system-upgrade` plugin package |
|---|---|---|
| ≤ 40 | `dnf` (DNF4) | `dnf-plugin-system-upgrade` |
| ≥ 41 | `dnf5` (DNF5) | Built-in to `dnf5-plugins` |

On Fedora 41+, the `dnf` command is a compatibility shim backed by DNF5. The `system-upgrade` subcommand is native to DNF5 and available without installing an additional plugin.

**DNF4 default behaviour for `system-upgrade download`:**  
Behaves like `distro-sync` but requires `--allow-downgrade` to handle packages from 3rd-party repositories (NVIDIA, Chrome, etc.) that have higher version numbers than those available in the target Fedora release. Without it, the offline transaction can fail at apply time.

**DNF5 default behaviour for `system-upgrade download`:**  
According to official DNF5 documentation, the default already behaves like `distro-sync` (always installs packages from the new release, even if older than the currently-installed version). The `--no-downgrade` flag would restrict this. `--allow-downgrade` is not a documented DNF5 flag (the default is equivalent).

**`dnf system-upgrade reboot` behaviour:**  
Documented in both DNF4 and DNF5 as: "Prepares the system to perform the offline transaction and **reboots** to start the transaction." It stores offline transaction data at `/usr/lib/sysimage/libdnf5/offline` (DNF5) or creates a `/system-update` symlink (DNF4), then calls `systemctl reboot`. This means it initiates a real system reboot as part of its normal execution.

**Sources:**
1. [DNF5 System-Upgrade Command Reference](https://dnf5.readthedocs.io/en/latest/commands/system-upgrade.8.html)
2. [DNF5 Offline Command Reference](https://dnf5.readthedocs.io/en/latest/commands/offline.8.html)
3. [systemd Offline System Updates Specification](https://www.freedesktop.org/wiki/Software/systemd/SystemUpdates/)
4. Fedora Quick Docs: Upgrading Fedora Offline (offline upgrade procedure)
5. `dnf-plugin-system-upgrade` upstream README (DNF4 plugin documentation)
6. Fedora 41 release notes (DNF5 as default)

---

## 2. Problem Definition

### Bug 1 — Missing `--allow-downgrade` flag (CRITICAL)

**Symptom:** User reboots, system applies zero changes, stays on Fedora 43.

**Root cause:** When third-party packages (e.g., NVIDIA drivers, Google Chrome, VS Code) have version numbers higher than those available in the target Fedora release, the offline upgrade transaction is **silently aborted** at boot time because DNF4 cannot resolve the dependency conflict without explicit permission to downgrade those packages.

The `dnf system-upgrade download` command exits `0` (success — packages were downloaded), but the actual upgrade transaction executed during the offline boot fails and rolls back, leaving the system on the original release.

**Affected versions:** Primarily DNF4 (Fedora ≤ 40). DNF5 defaults to `distro-sync` behaviour (downgrading allowed by default), so this flag is not strictly required on Fedora 41+. However, it is documented as a best practice in official Fedora upgrade guides and is harmless on DNF5.

**Current code:**
```rust
&["dnf", "system-upgrade", "download", "--releasever", &ver_str, "-y"],
```

**Fix required:** Add `--allow-downgrade`:
```rust
&["dnf", "system-upgrade", "download", "--releasever", &ver_str, "--allow-downgrade", "-y"],
```

---

### Bug 2 — `dnf system-upgrade reboot` incorrectly awaited (CRITICAL)

**Symptom:** Two failure modes depending on execution environment:

**Mode A — Reboot succeeds:**
1. `dnf system-upgrade reboot` sets up the offline upgrade mechanism (creates `/system-update` symlink or writes DNF5 state) **and immediately calls `systemctl reboot`**.
2. systemd begins shutdown; pkexec is killed by SIGTERM.
3. `child.wait()` in `run_command_sync` receives a non-zero exit code (signal termination).
4. `run_command_sync` returns `false`.
5. `upgrade_fedora` returns `Err("Failed to trigger Fedora upgrade reboot")`.
6. `execute_upgrade` propagates the error.
7. The UI shows "Upgrade failed" instead of the reboot dialog.
8. The system reboots anyway and runs the offline upgrade correctly — but the user sees an error and panic about what happened.

**Mode B — Reboot does not trigger (D-Bus unavailable in pkexec context):**
1. `dnf system-upgrade reboot` sets up the offline state, but `systemctl reboot` fails silently inside the pkexec context (no D-Bus session).
2. `dnf system-upgrade reboot` exits `0`.
3. `run_command_sync` returns `true`.
4. `upgrade_fedora` returns `Ok(())`.
5. UI shows the reboot dialog.
6. User clicks reboot in the dialog → `src/reboot.rs::reboot()` calls `systemctl reboot` as the regular user.
7. **The offline upgrade mechanism was set up, so the offline upgrade DOES run on next boot.** This path works.

However, in Mode B it is also possible that `systemctl reboot` inside pkexec also fails partially, or that the offline state was not properly written, in which case the system reboots normally and the upgrade is not applied.

The core architectural issue: `dnf system-upgrade reboot` **is designed to trigger a reboot** as part of its own execution. It must not be awaited with `run_command_sync`. Instead, it should be spawned fire-and-forget. A status message should be sent to the log and the function should return `Ok(())` immediately to allow the UI to display the final status to the user.

**Current code:**
```rust
if !crate::runner::run_command_sync("pkexec", &["dnf", "system-upgrade", "reboot"], tx) {
    return Err("Failed to trigger Fedora upgrade reboot (see log for details)".to_string());
}
Ok(())
```

**Fix required:** Replace with a fire-and-forget `spawn()`:
```rust
let _ = tx.send_blocking("Download complete. Scheduling upgrade for next reboot...".into());
use std::process::Stdio;
match std::process::Command::new("pkexec")
    .args(["dnf", "system-upgrade", "reboot"])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
{
    Ok(_child) => {
        let _ = tx.send_blocking(
            "Upgrade reboot triggered. The system will restart to apply the upgrade.".into(),
        );
        Ok(())
    }
    Err(e) => Err(format!("Failed to start upgrade reboot: {e}")),
}
```

This ensures:
- `upgrade_fedora()` returns `Ok(())` immediately after spawning the reboot.
- The UI receives the `Ok(())` result and shows the reboot dialog.
- The spawned `pkexec dnf system-upgrade reboot` process proceeds in the background, setting up the offline transaction and triggering the reboot.
- If the system reboots before the user interacts with the dialog, that is expected and correct.

---

### Bug 3 — Wrong plugin package name for Fedora 41+ (MODERATE)

**Symptom:** Plugin installation step emits an error on Fedora 41+ systems, but the upgrade continues anyway (or fails at a confusing point).

**Root cause:** On Fedora 41+, `dnf` is DNF5. The system-upgrade plugin for DNF4 is `dnf-plugin-system-upgrade`. On DNF5, system-upgrade is part of `dnf5-plugins`. The package `dnf-plugin-system-upgrade` does not exist on Fedora 41+.

However, in practice `dnf5-plugins` is already installed on most Fedora 41+ systems (it ships by default), so the plugin step fails but the actual `dnf system-upgrade download` command still works. This is why users see the multiple-sudo-prompt behaviour and download progress — the download works even though the plugin install step failed.

**Fix approach:** Use a best-effort installation of `dnf5-plugin-system-upgrade` and ignore failure. Since `system-upgrade` is nearly always available on modern Fedora and DNF4 systems where it is not, the `dnf-plugin-system-upgrade` install is also best-effort. The function should not abort if the install step fails.

**Current code:**
```rust
let _ = tx.send_blocking("Installing system-upgrade plugin...".into());
if !crate::runner::run_command_sync(
    "pkexec",
    &["dnf", "install", "-y", "dnf-plugin-system-upgrade"],
    tx,
) {
    return Err(
        "Failed to install dnf-plugin-system-upgrade (see log for details)".to_string(),
    );
}
```

**Fix required:** Change to best-effort (ignore failure), use `dnf5-plugin-system-upgrade`:
```rust
// Best-effort: ensure system-upgrade plugin is available.
// On Fedora ≥ 41 (DNF5), system-upgrade is part of dnf5-plugins (usually pre-installed).
// On Fedora ≤ 40 (DNF4), the package is dnf-plugin-system-upgrade.
// Either may already be installed; failure here is non-fatal.
let _ = tx.send_blocking("Ensuring system-upgrade plugin is available...".into());
let _ = crate::runner::run_command_sync(
    "pkexec",
    &["dnf", "install", "-y", "dnf5-plugin-system-upgrade"],
    tx,
);
```

---

## 3. Proposed Solution Architecture

### Changes confined to: `src/upgrade.rs`

No changes required to `src/runner.rs`, `src/ui/upgrade_page.rs`, or `src/reboot.rs`.

The three fixes are all local to the `upgrade_fedora()` function.

### Updated `upgrade_fedora()` function

```rust
fn upgrade_fedora(tx: &async_channel::Sender<String>) -> Result<(), String> {
    // Step 1: Ensure system-upgrade plugin is available (best-effort).
    // On Fedora ≥ 41 (DNF5), system-upgrade is part of dnf5-plugins (usually pre-installed).
    // On Fedora ≤ 40 (DNF4), the package is dnf-plugin-system-upgrade.
    // Failure here is non-fatal — dnf system-upgrade may already be available.
    let _ = tx.send_blocking("Ensuring system-upgrade plugin is available...".into());
    let _ = crate::runner::run_command_sync(
        "pkexec",
        &["dnf", "install", "-y", "dnf5-plugin-system-upgrade"],
        tx,
    );

    // Step 2: Download upgrade packages for the next Fedora release.
    // --allow-downgrade: required when 3rd-party repos (NVIDIA, Chrome, etc.) contain
    // packages with version numbers higher than those in the target Fedora release.
    // Without this flag, the offline transaction can silently fail at boot time,
    // leaving the system on the current release with no visible error.
    let _ = tx.send_blocking("Downloading upgrade packages...".into());

    let next_version = match detect_next_fedora_version() {
        Some(v) => v,
        None => {
            let _ = tx.send_blocking(
                "Error: Could not detect current Fedora version. Aborting upgrade.".into(),
            );
            return Err(
                "Could not detect current Fedora version to determine upgrade target".to_string(),
            );
        }
    };
    let ver_str = next_version.to_string();

    if !crate::runner::run_command_sync(
        "pkexec",
        &[
            "dnf",
            "system-upgrade",
            "download",
            "--releasever",
            &ver_str,
            "--allow-downgrade",
            "-y",
        ],
        tx,
    ) {
        return Err(format!(
            "Failed to download Fedora {} upgrade packages (see log for details)",
            next_version
        ));
    }

    // Step 3: Trigger reboot into offline upgrade.
    // dnf system-upgrade reboot sets up the systemd offline-upgrade mechanism and
    // immediately calls systemctl reboot. It must NOT be awaited via run_command_sync:
    // if the reboot succeeds, pkexec is killed by SIGTERM before it can exit cleanly,
    // which would cause run_command_sync to return false and report a spurious error.
    let _ = tx.send_blocking("Download complete. Scheduling upgrade for next reboot...".into());
    use std::process::Stdio;
    match std::process::Command::new("pkexec")
        .args(["dnf", "system-upgrade", "reboot"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(_child) => {
            let _ = tx.send_blocking(
                "Upgrade reboot triggered. The system will restart to apply the upgrade.".into(),
            );
            Ok(())
        }
        Err(e) => Err(format!("Failed to start upgrade reboot: {e}")),
    }
}
```

---

## 4. Exact Code Changes to `src/upgrade.rs`

### Change 1: Step 1 — Plugin installation (best-effort, updated package name)

**Remove:**
```rust
    // Step 1: Install upgrade plugin
    let _ = tx.send_blocking("Installing system-upgrade plugin...".into());
    if !crate::runner::run_command_sync(
        "pkexec",
        &["dnf", "install", "-y", "dnf-plugin-system-upgrade"],
        tx,
    ) {
        return Err(
            "Failed to install dnf-plugin-system-upgrade (see log for details)".to_string(),
        );
    }
```

**Replace with:**
```rust
    // Step 1: Ensure system-upgrade plugin is available (best-effort).
    // On Fedora ≥ 41 (DNF5), system-upgrade is part of dnf5-plugins (usually pre-installed).
    // On Fedora ≤ 40 (DNF4), the package is dnf-plugin-system-upgrade.
    // Failure here is non-fatal — dnf system-upgrade may already be available.
    let _ = tx.send_blocking("Ensuring system-upgrade plugin is available...".into());
    let _ = crate::runner::run_command_sync(
        "pkexec",
        &["dnf", "install", "-y", "dnf5-plugin-system-upgrade"],
        tx,
    );
```

### Change 2: Step 2 — Add `--allow-downgrade` flag

**Remove:**
```rust
    if !crate::runner::run_command_sync(
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

**Replace with:**
```rust
    if !crate::runner::run_command_sync(
        "pkexec",
        &[
            "dnf",
            "system-upgrade",
            "download",
            "--releasever",
            &ver_str,
            "--allow-downgrade",
            "-y",
        ],
        tx,
    ) {
```

### Change 3: Step 3 — Fire-and-forget reboot trigger

**Remove:**
```rust
    // Step 3: Trigger reboot into upgrade
    let _ =
        tx.send_blocking("Download complete. The system will reboot to apply the upgrade.".into());
    if !crate::runner::run_command_sync("pkexec", &["dnf", "system-upgrade", "reboot"], tx) {
        return Err("Failed to trigger Fedora upgrade reboot (see log for details)".to_string());
    }
    Ok(())
```

**Replace with:**
```rust
    // Step 3: Trigger reboot into offline upgrade.
    // dnf system-upgrade reboot sets up the systemd offline-upgrade mechanism and
    // immediately calls systemctl reboot. It must NOT be awaited via run_command_sync:
    // if the reboot succeeds, pkexec is killed by SIGTERM before it can exit cleanly,
    // which would cause run_command_sync to return false and report a spurious error.
    let _ = tx.send_blocking("Download complete. Scheduling upgrade for next reboot...".into());
    use std::process::Stdio;
    match std::process::Command::new("pkexec")
        .args(["dnf", "system-upgrade", "reboot"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(_child) => {
            let _ = tx.send_blocking(
                "Upgrade reboot triggered. The system will restart to apply the upgrade.".into(),
            );
            Ok(())
        }
        Err(e) => Err(format!("Failed to start upgrade reboot: {e}")),
    }
```

---

## 5. Implementation Steps

1. Open `src/upgrade.rs`.
2. Locate the `upgrade_fedora()` function (search for `fn upgrade_fedora`).
3. Apply Change 1: replace the plugin install block.
4. Apply Change 2: add `--allow-downgrade` to the args array.
5. Apply Change 3: replace the `run_command_sync` reboot block with the fire-and-forget `spawn()` block.
6. Verify that `use std::process::Stdio;` does not conflict with any existing imports in the function (it is a local `use` inside the match arm, which is valid Rust).
7. Run `cargo build` — must compile without errors.
8. Run `cargo clippy -- -D warnings` — must produce no warnings.
9. Run `cargo fmt --check` — must pass with no formatting diffs.

---

## 6. Dependencies

No new Rust crates or external dependencies are required. All changes use:

- `std::process::Command` — already in scope via `use std::process::Command` at top of `src/upgrade.rs`.
- `std::process::Stdio` — added as a local `use` inside the match arm (or can be added to the top-level imports of the function).
- `async_channel::Sender<String>` — already used throughout `upgrade_fedora`.
- `crate::runner::run_command_sync` — already used throughout `upgrade_fedora`.

---

## 7. Risks and Mitigations

### Risk 1: `--allow-downgrade` not recognised on DNF5

**Likelihood:** Low. The `dnf` compatibility wrapper on Fedora 41+ generally accepts DNF4 flags and maps them or ignores unknown ones gracefully.

**Mitigation:** The flag is documented in the official Fedora offline upgrade guide and was used in official Fedora documentation for many release cycles. If DNF5 does not recognise it, it will print a warning to stderr (visible in the log panel) but should not abort. Testing on a Fedora 41+ system is recommended before release.

**Fallback:** If `--allow-downgrade` causes a hard failure on DNF5, it can be replaced with no flag (DNF5 defaults to distro-sync behaviour). A version-detection gate can be added later if necessary.

### Risk 2: Fire-and-forget spawn leaves zombie child process

**Likelihood:** Low. `_child` is dropped immediately after `spawn()` returns, and the OS will reap the process automatically once the parent (`up`) exits (or the system reboots).

**Mitigation:** Acceptable: the parent process (`up`) will be killed by systemd during shutdown anyway. No action needed.

### Risk 3: `dnf system-upgrade reboot` fails to set up offline state before the process is spawned

**Likelihood:** Very low. The `spawn()` call returns once the process is started by the OS; setup happens within the child process synchronously before `systemctl reboot` is called.

**Mitigation:** If the child process fails to set up the offline state (e.g., due to a dnf database lock), it will exit without rebooting and the user will see the reboot dialog. Clicking reboot in the dialog will execute a normal reboot without the upgrade. No data loss occurs; the user can retry.

### Risk 4: Plugin install (`dnf5-plugin-system-upgrade`) fails on Fedora ≤ 40

**Likelihood:** High — this package does not exist on Fedora ≤ 40.

**Mitigation:** The install step is now best-effort (result ignored). `dnf-plugin-system-upgrade` is still typically installed on Fedora ≤ 40 by default. If it is missing, `dnf system-upgrade download` in Step 2 will report the missing plugin and Step 2 will return an appropriate error.

### Risk 5: System reboots before user sees the reboot dialog

**Likelihood:** Medium to high on fast hardware.

**Mitigation:** This is expected and correct behaviour. The reboot is triggered by `dnf system-upgrade reboot` itself; the dialog is informational. If the system reboots before the user can see the dialog, the upgrade will still proceed correctly offline.

---

## 8. Testing Guidance

Since no automated tests currently exist for the upgrade flow (as noted in the project constraints), manual testing steps are:

1. **DNF4 system (Fedora 40):**
   - Confirm `dnf5-plugin-system-upgrade` install step fails silently (logged but no abort).
   - Confirm `--allow-downgrade` is accepted by `dnf system-upgrade download`.
   - Confirm Step 3 spawns the process and returns `Ok(())`.
   - Confirm the reboot dialog appears.

2. **DNF5 system (Fedora 41, 42, 43):**
   - Confirm `dnf5-plugin-system-upgrade` install step succeeds (or is a no-op if already installed).
   - Confirm `--allow-downgrade` does not cause a hard failure.
   - Confirm Step 3 spawns the process and returns `Ok(())`.
   - Confirm the reboot dialog appears.
   - Confirm the system reboots into the offline upgrade environment.
   - Confirm the system upgrades to the next Fedora release.

3. **Regression: 3rd-party package scenario:**
   - Install a package from a 3rd-party repo with a high version number before upgrading.
   - Confirm the offline upgrade completes successfully with the new `--allow-downgrade` flag.
