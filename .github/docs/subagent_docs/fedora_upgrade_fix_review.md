# Review: Fedora Upgrade Bug Fix

**Feature name:** `fedora_upgrade_fix`  
**Date:** 2026-05-03  
**Reviewer:** Code Review Agent  
**Status:** PASS  

---

## Summary of Findings

The implementation in `src/upgrade.rs` correctly addresses all three bugs documented in the spec. A single formatting issue (`cargo fmt --check` failure) was detected and corrected during this review before final validation. All four build commands now pass cleanly.

---

## Bug Fix Verification

### Bug 1 — `--allow-downgrade` flag ✅

The `dnf system-upgrade download` call in Step 2 now includes `--allow-downgrade`:

```rust
&[
    "dnf",
    "system-upgrade",
    "download",
    "--releasever",
    &ver_str,
    "--allow-downgrade",
    "-y",
],
```

This prevents silent offline transaction failures caused by 3rd-party packages (NVIDIA, Chrome, VS Code, etc.) whose version numbers exceed those available in the target Fedora release. Without this flag, DNF4 aborts the transaction at boot time and leaves the system on the original release with no visible error to the user.

---

### Bug 2 — Fire-and-forget reboot spawn ✅

Step 3 now uses `std::process::Command::spawn()` with all I/O handles set to `Stdio::null()`:

```rust
match std::process::Command::new("pkexec")
    .args(["dnf", "system-upgrade", "reboot"])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
{
    Ok(_child) => { ... Ok(()) }
    Err(e) => Err(format!("Failed to start upgrade reboot: {e}")),
}
```

`upgrade_fedora()` returns `Ok(())` immediately after a successful spawn. This allows the UI to display the reboot dialog regardless of whether the system reboots before the user can interact with it. The previous `run_command_sync` approach caused a spurious `Err` when the reboot succeeded (because systemd's SIGTERM killed pkexec before it could exit cleanly).

---

### Bug 3 — Best-effort plugin installation with DNF4 fallback ✅

Step 1 now:
1. Attempts `dnf5-plugin-system-upgrade` (DNF5 / Fedora 41+) — result discarded on failure.
2. Falls back to `dnf-plugin-system-upgrade` (DNF4 / Fedora ≤ 40) — result also discarded.
3. Never returns `Err` from the plugin step.

```rust
if !crate::runner::run_command_sync(
    "pkexec",
    &["dnf", "install", "-y", "dnf5-plugin-system-upgrade"],
    tx,
) {
    let _ = tx.send_blocking(
        "dnf5-plugin-system-upgrade not found; trying dnf-plugin-system-upgrade...".into(),
    );
    // Ignore failure — the plugin is typically already present.
    let _ = crate::runner::run_command_sync(
        "pkexec",
        &["dnf", "install", "-y", "dnf-plugin-system-upgrade"],
        tx,
    );
}
```

This exceeds the spec's single-attempt proposal by providing an explicit DNF4 fallback, which is strictly better.

---

## Code Review

### Style Consistency

One minor inconsistency: Step 3 uses `std::process::Command::new("pkexec")` with the fully qualified path, while `Command` is already imported at the top of the file (`use std::process::Command;`). The code could use `Command::new("pkexec")` instead. Similarly, `use std::process::Stdio;` is declared inside the function body rather than at the module level with the other imports.

Neither issue triggers a compiler warning or clippy lint, and the spec itself shows this pattern in the proposed code. Impact is cosmetic.

### Error Messages

Error messages are clear and user-facing:
- `"Failed to start upgrade reboot: {e}"` — includes the OS error.
- `"Failed to download Fedora {N} upgrade packages (see log for details)"` — includes the version.

### Comments

All three steps have substantive inline comments explaining _why_ the implementation is written the way it is (the reasoning about SIGTERM, the DNF4/DNF5 split, `--allow-downgrade` semantics). This is well above average for this codebase.

---

## Security Review

| Check | Result |
|---|---|
| Command injection | No user input reaches any arg; all args are static strings or `u32::to_string()` |
| Privilege escalation | pkexec used correctly for all root operations |
| Spawned child I/O | `Stdio::null()` on all three handles — no dangling pipes |
| Version detection | `detect_next_fedora_version()` returns `Option<u32>`; version used only after `u32::to_string()` |

No security issues found.

---

## Build Validation

| Command | Result |
|---|---|
| `cargo fmt --check` | ✅ PASS (one formatting diff corrected before final run) |
| `cargo build` | ✅ PASS (compiled in 4.88s) |
| `cargo clippy -- -D warnings` | ✅ PASS (no warnings) |
| `cargo test` | ✅ PASS (18 tests, 0 failures) |

---

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 97% | A+ |
| Best Practices | 90% | A- |
| Functionality | 100% | A+ |
| Code Quality | 90% | A- |
| Security | 100% | A+ |
| Performance | 100% | A+ |
| Consistency | 85% | B+ |
| Build Success | 100% | A+ |

**Overall Grade: A (95%)**

---

## Issues Found

### CORRECTED (during review)
- **Formatting**: `cargo fmt --check` reported a diff — one `tx.send_blocking(...)` call was line-wrapped when it fit on a single line. Fixed by collapsing to a single line. No logic change.

### MINOR (no action required)
- `std::process::Command::new` uses the full path inside the function instead of the already-imported `Command` alias — cosmetic only, no clippy lint.
- `use std::process::Stdio;` is inside the function body rather than at the top of the file — consistent with the spec's proposed code; acceptable in Rust.

---

## Verdict

**PASS**

All three bugs are correctly fixed. The implementation matches or exceeds the specification. The build is clean across all four validation commands. The formatting issue was found and resolved during this review pass.
