# Specification: Hostname Validation in `upgrade_nixos()` (Flake Branch)

**Feature Name:** `hostname_validation`
**Severity:** Critical — Security Bug Fix
**Affected File:** `src/upgrade.rs`
**Reference File (validated counterpart):** `src/backends/nix.rs`

---

## 1. Current State Analysis

### 1.1 `detect_hostname()` — `src/upgrade.rs` lines 36–40

```rust
pub fn detect_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .unwrap_or_else(|_| "nixos".to_owned())
        .trim()
        .to_string()
}
```

This function reads `/proc/sys/kernel/hostname`, strips leading/trailing whitespace with
`.trim()`, and returns the result as an owned `String`. It performs **no character
validation**. Any byte sequence that can appear in that file — including `#`, `?`, space,
NUL (`\0`), newline (`\n`), or arbitrary Unicode — is returned verbatim.

### 1.2 `validate_hostname()` — `src/backends/nix.rs` lines 28–42

```rust
fn validate_hostname(hostname: &str) -> Result<&str, String> {
    if hostname.is_empty() {
        return Err("hostname is empty".to_string());
    }
    if hostname.len() > 253 {
        return Err(format!(
            "hostname is too long ({} chars, max 253)",
            hostname.len()
        ));
    }
    if !hostname
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(format!("Invalid hostname: {:?}", hostname));
    }
    Ok(hostname)
}
```

This private function enforces three invariants:

| Check | Condition |
|---|---|
| Non-empty | `hostname.is_empty()` → `Err` |
| Length | `hostname.len() > 253` → `Err` |
| Character whitelist | Only ASCII alphanumeric, `-`, `_`, `.` → else `Err` |

Permitted characters explicitly include **underscore (`_`) and dot (`.`)**, both of which
are common in real NixOS host names (e.g., `my_workstation`, `nixos.local`). The function
therefore neither over- nor under-validates for practical NixOS deployments.

### 1.3 Call sites that already use `validate_hostname()` — `src/backends/nix.rs`

`validate_hostname()` is invoked at **one call site** in `nix.rs`, inside
`<NixBackend as Backend>::run_update()` at approximately line 71:

```rust
let raw_hostname = nixos_hostname();
let hostname = match validate_hostname(&raw_hostname) {
    Ok(h) => h,
    Err(e) => return UpdateResult::Error(e),
};
```

This covers the **routine package-update** code path (triggered by the Updates tab).

### 1.4 Vulnerable location — `src/upgrade.rs`, `upgrade_nixos()`, `NixOsConfigType::Flake` branch

Inside `fn upgrade_nixos(tx: &async_channel::Sender<String>) -> bool`, the `Flake` match
arm (approximately lines 457–470) contains:

```rust
NixOsConfigType::Flake => {
    // ...
    let hostname = detect_hostname();                        // ← unvalidated
    let flake_target = format!("/etc/nixos#{}", hostname);  // ← injected into flake ref
    let _ = tx.send_blocking(format!(
        "Rebuilding NixOS configuration: {flake_target}"
    ));
    run_streaming_command(
        "pkexec",
        &["nixos-rebuild", "switch", "--flake", &flake_target],
        tx,
    )
}
```

`validate_hostname()` is **never called** in this branch. The raw hostname is embedded
directly into the flake reference and passed to `nixos-rebuild`.

---

## 2. Problem Definition

### 2.1 Characters the missing validation fails to reject

`detect_hostname()` trims ASCII whitespace but rejects nothing else. Characters that can
appear in `/proc/sys/kernel/hostname` and are **not** in the `validate_hostname()`
whitelist include (non-exhaustive):

| Character | Risk |
|---|---|
| `#` | Nix flake URI delimiter — a second `#` splits the fragment again, corrupting the host reference |
| `?` | Nix flake URI query separator — may truncate or reinterpret the flake path |
| Space (` `) | Argument boundary ambiguity; the argument is passed as a single `argv` element, but Nix itself may parse the space as a separator within the URI string |
| Newline (`\n`) | May be read from the file before `.trim()` strips the trailing one; internal embedded newlines are not stripped by `.trim()` and would corrupt the argument |
| NUL byte (`\0`) | Causes premature string termination in C-based Nix tooling |
| Non-ASCII Unicode | Undefined behaviour in Nix flake URI parsing; may panic or silently resolve to a wrong target |

### 2.2 Runtime risk

The flake reference `/etc/nixos#<hostname>` is passed as the value of `--flake` to
`nixos-rebuild switch`, which is executed (via `pkexec`) with **root privileges**. The Nix
flake resolver interprets the fragment after `#` as a NixOS configuration attribute name.

A hostname containing `#foo` would result in `/etc/nixos#<real>#foo`, causing Nix to
attempt to evaluate an attribute named `<real>#foo` (or fail unpredictably). A hostname
containing characters that Nix's URI parser treats as syntax could:

- cause the rebuild to silently target a **different** NixOS configuration attribute
- cause `nixos-rebuild` to fail with a confusing or misleading error
- in adversarial conditions (e.g., if the hostname file is writable by a lower-privileged
  process or an initramfs script before the kernel sanitises it), redirect the privileged
  rebuild to an attacker-controlled flake output

### 2.3 Why the inconsistency is dangerous

The two code paths — `nix.rs::run_update()` (Updates tab) and `upgrade.rs::upgrade_nixos()`
(Upgrade tab) — perform structurally identical operations: embed a hostname into a Nix
flake URI and pass it to a privileged `nixos-rebuild` invocation. The backend path
validates; the upgrade path does not.

This inconsistency means:

1. A hostname that is **silently accepted** by the upgrade path would be **explicitly
   rejected** by the update path — and vice versa — producing inconsistent, surprising
   behaviour.
2. Security audits or automated scanners that inspect `nix.rs` and conclude "hostname is
   validated" will miss the parallel code path in `upgrade.rs`.
3. Any future refactor that assumes the hostname has already been validated (because it is
   validated "everywhere else") will silently break the security invariant.

---

## 3. Proposed Solution Architecture

### 3.1 Add a private `validate_hostname()` to `src/upgrade.rs`

A private function with **identical logic to the one in `nix.rs`** must be added to
`src/upgrade.rs`, placed immediately after `detect_hostname()`.

This is an **intentional duplication**. `upgrade.rs` must not import a private validation
helper from a backend submodule (`src/backends/nix.rs`), as backends are not a public
API surface of the application core. If deduplication is desired in the future, the
correct architectural approach is a dedicated `src/validation.rs` module (or a `utils`
submodule) that both files import from. That refactor is **out of scope** for this fix.

### 3.2 New function body

```rust
/// Validates that a hostname is safe to use as a NixOS flake output attribute.
/// Only ASCII alphanumeric, hyphen, underscore, and dot are permitted.
/// This is intentionally identical to the copy in `src/backends/nix.rs`.
/// If deduplication is needed, move both copies to `src/validation.rs`.
fn validate_hostname(hostname: &str) -> Result<&str, String> {
    if hostname.is_empty() {
        return Err("hostname is empty".to_string());
    }
    if hostname.len() > 253 {
        return Err(format!(
            "hostname is too long ({} chars, max 253)",
            hostname.len()
        ));
    }
    if !hostname
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(format!("Invalid hostname: {:?}", hostname));
    }
    Ok(hostname)
}
```

### 3.3 Fix the `NixOsConfigType::Flake` branch in `upgrade_nixos()`

Replace the two unvalidated lines:

```rust
// BEFORE (vulnerable)
let hostname = detect_hostname();
let flake_target = format!("/etc/nixos#{}", hostname);
```

With the validated version:

```rust
// AFTER (fixed)
let raw_hostname = detect_hostname();
let hostname = match validate_hostname(&raw_hostname) {
    Ok(h) => h,
    Err(e) => {
        let _ = tx.send_blocking(format!("Error: invalid hostname — {e}"));
        return false;
    }
};
let flake_target = format!("/etc/nixos#{}", hostname);
```

The error message is sent through `tx` (the async channel to the UI log panel) and the
function returns `false`, consistent with how all other failure modes in `upgrade_nixos()`
are handled.

---

## 4. Implementation Steps

The following steps must be executed **in order** by the implementation agent.

1. **Read** `src/upgrade.rs` lines 36–45 (surrounding `detect_hostname()`) and lines
   440–475 (the `upgrade_nixos()` function body, specifically the `NixOsConfigType::Flake`
   arm) to confirm exact line numbers and current indentation.

2. **Insert** the `validate_hostname()` function as a private (`fn`, not `pub fn`) function
   in `src/upgrade.rs`, immediately after the closing `}` of `detect_hostname()`. The
   function body must be **byte-for-byte identical** to the copy in `src/backends/nix.rs`
   (lines 28–42), with the deduplication note added as a doc comment (see §3.2 above).

3. **Replace** the two vulnerable lines in the `NixOsConfigType::Flake` branch of
   `upgrade_nixos()` with the validated version shown in §3.3. Do not modify any other
   part of `upgrade_nixos()`.

4. **Add** a `#[cfg(test)] mod tests` block at the **bottom** of `src/upgrade.rs`
   containing unit tests for `validate_hostname()`. The test block must cover:

   | Test case | Input | Expected result |
   |---|---|---|
   | Valid simple hostname | `"nixos"` | `Ok("nixos")` |
   | Valid hostname with hyphen | `"my-machine"` | `Ok("my-machine")` |
   | Valid hostname with underscore | `"my_workstation"` | `Ok(...)` |
   | Valid hostname with dot | `"nixos.local"` | `Ok(...)` |
   | Empty hostname | `""` | `Err(...)` |
   | Hostname exceeding 253 chars | `"a".repeat(254)` | `Err(...)` |
   | Hostname containing `#` | `"host#evil"` | `Err(...)` |
   | Hostname containing `?` | `"host?q=1"` | `Err(...)` |
   | Hostname containing space | `"host name"` | `Err(...)` |
   | Hostname containing NUL byte | `"host\0null"` | `Err(...)` |
   | Hostname containing newline | `"host\nnewline"` | `Err(...)` |

   Every test must use `assert_eq!` or `assert!(result.is_ok())` / `assert!(result.is_err())`
   with a descriptive message.

---

## 5. Dependencies

- **No new Cargo dependencies required.**
- **No `Cargo.toml` changes.**

The fix uses only the Rust standard library (`str`, `char`) — the same primitives already
used by the existing `validate_hostname()` in `nix.rs`.

---

## 6. Configuration Changes

None. No build scripts, Meson files, Nix flake, or Flatpak manifests require modification.

---

## 7. Risks and Mitigations

### Risk 1: Local copy diverges from `nix.rs` over time

If `validate_hostname()` in `nix.rs` is later strengthened (e.g., to reject leading
hyphens, enforce label length limits, or disallow dotless names) and the copy in
`upgrade.rs` is not updated in parallel, the two code paths will silently enforce different
rules.

**Mitigation:**

- Both copies carry a doc comment explicitly stating they are intentional duplicates and
  identifying the canonical deduplication path (`src/validation.rs`).
- Both copies have **identical unit test suites** (Step 4 above matches the test coverage
  expected in `nix.rs`). Any future tightening of the validation logic in one file will
  require the same change in the other to keep all tests green — making divergence
  immediately visible.

### Risk 2: A valid NixOS hostname is incorrectly rejected

Some administrators use underscores (e.g., `my_workstation`) or dots (e.g., `nixos.local`)
in their NixOS host names, which are not valid in DNS hostnames but are valid and common as
NixOS `networking.hostName` values.

**Mitigation:**

`validate_hostname()` in `nix.rs` already permits `_` and `.` in addition to ASCII
alphanumeric and `-`. The confirmed character-class check from `nix.rs` line 35 is:

```rust
c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'
```

Both underscores and dots are explicitly allowed. The copy in `upgrade.rs` must be
identical, preserving this permissive-but-safe whitelist. No legitimate NixOS host name
(as set by `networking.hostName` in a NixOS configuration) requires characters outside
this set.

### Risk 3: The `tx.send_blocking` error path is silently ignored at the call site

`upgrade_nixos()` returns `false` on validation failure, which propagates through
`execute_upgrade()` back to the UI. However, the UI must display the error message from
`tx` rather than silently failing.

**Mitigation:** This is the existing error-propagation contract throughout `upgrade.rs`:
`run_streaming_command()` already sends error messages through `tx` before returning
`false`, and the UI log panel is subscribed to `tx`. The validation error message sent via
`tx.send_blocking()` follows the same contract and will appear in the log panel without
any additional UI changes.
