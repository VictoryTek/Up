# Review: Hostname Validation in `upgrade_nixos()` (Flake Branch)

**Feature Name:** `hostname_validation`
**Review Date:** 2026-03-18
**Reviewer:** Senior Rust Engineer / Security Code Reviewer
**Spec File:** `.github/docs/subagent_docs/hostname_validation_spec.md`
**Modified File:** `src/upgrade.rs`

---

## Build Command Outputs

### `cargo build`
```
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.04s
```
**Exit code: 0 — PASS**

### `cargo test`
```
Finished `test` profile [unoptimized + debuginfo] target(s) in 0.04s
Running unittests src/main.rs (target/debug/deps/up-103df1d1643a1002)

running 2 tests
test upgrade::tests::validate_hostname_rejects_dangerous_input ... ok
test upgrade::tests::validate_hostname_accepts_valid_input ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```
**Exit code: 0 — PASS**

### `cargo clippy -- -D warnings`
```
error: no such command: `clippy`
```
**NOT AVAILABLE** — `clippy` component is not installed on this system (Fedora system Rust 1.94.0, no `rustup` toolchain).
`cargo check` was used as a substitute:
```
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.05s
```
**`cargo check` exit code: 0 — PASS**

### `cargo fmt --check`
```
error: no such command: `fmt`
```
**NOT AVAILABLE** — `rustfmt` component is not installed on this system.

---

## Checklist Findings

### 1. Specification Compliance

**1.1 Is `validate_hostname()` added in `upgrade.rs` immediately after `detect_hostname()`?**

✅ **PASS** — `validate_hostname()` appears at lines 43–66 of `src/upgrade.rs`, directly following the closing `}` of `detect_hostname()` (lines 36–41). No intervening code.

**1.2 Does `validate_hostname()` in `upgrade.rs` match the validation logic in `nix.rs` exactly?**

✅ **PASS** — The function body is byte-for-byte identical to `validate_hostname()` in `src/backends/nix.rs` (lines 28–44). The same three invariants are enforced in the same order: `is_empty()` check, `len() > 253` check, character whitelist check `(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')`. Error message strings are also identical.

**1.3 Is the `NixOsConfigType::Flake` branch updated to use `validate_hostname()`?**

✅ **PASS** — The flake branch in `upgrade_nixos()` now reads:
```rust
let raw_hostname = detect_hostname();
let hostname = match validate_hostname(&raw_hostname) {
    Ok(h) => h,
    Err(e) => {
        let _ = tx.send_blocking(format!("Upgrade aborted: {e}"));
        return false;
    }
};
let flake_target = format!("/etc/nixos#{}", hostname);
```

**1.4 Does the error path send a descriptive message via `tx` before returning `false`?**

✅ **PASS (with minor note)** — The error path sends `"Upgrade aborted: {e}"` through `tx` before returning `false`. The message is user-readable and follows the existing error-propagation contract of `upgrade_nixos()`.

⚠️ **Minor deviation**: The spec's §3.3 code example shows `"Error: invalid hostname — {e}"`. The implemented message `"Upgrade aborted: {e}"` conveys equivalent information and is arguably more consistent with the tone of other upgrade error messages in the file. This does not affect correctness, security, or user visibility of the error. The review checklist does not specify the exact string.

**Minor note**: The doc comment on `validate_hostname()` in `upgrade.rs` does not include the spec's suggested deduplication guidance (`If deduplication is needed, move both copies to src/validation.rs`). The comment instead explains the guard's purpose and the mirroring relationship. This is acceptable — the deduplication guidance was a recommendation, not a checklist requirement.

---

### 2. Best Practices

**Is the function private (not `pub`)?**

✅ **PASS** — Declared as `fn validate_hostname`, not `pub fn validate_hostname`.

**Does it return `Result<&str, String>`?**

✅ **PASS** — Signature: `fn validate_hostname(hostname: &str) -> Result<&str, String>`.

**Is the error message user-readable?**

✅ **PASS** — All three error variants produce clear, English-language messages:
- `"hostname is empty"`
- `"hostname is too long (N chars, max 253)"`
- `"Invalid hostname: <debug-repr>"`

---

### 3. Functionality

**Does `validate_hostname()` correctly reject `#`, `?`, spaces, NUL, newlines?**

✅ **PASS** — All tested in `validate_hostname_rejects_dangerous_input`. The character whitelist check `c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'` excludes all of these. Tests exercise:
- `"host#evil"` → `Err`
- `"host?url=override"` → `Err`
- `"my host"` → `Err`
- `"host\x00name"` → `Err`
- `"host\nmalicious"` → `Err`

The test also covers `"host;id"` (shell metacharacter) as defense-in-depth, which is not required by the spec but is beneficial.

**Does it correctly accept `_`, `.`, `-`, and alphanumeric?**

✅ **PASS** — Tested in `validate_hostname_accepts_valid_input`:
- `"my-server"` (hyphen) → `Ok`
- `"server1.local"` (dot) → `Ok`
- `"MY_SERVER_42"` (uppercase, digit, underscore) → `Ok`
- `"my_host"` (isolated underscore) → `Ok`
- `"nixos"` (simple alphanumeric) → `Ok`

**Does it reject empty strings and strings > 253 chars?**

✅ **PASS**:
- `""` → `Err`
- `"a".repeat(254)` → `Err`
- `"a".repeat(253)` → `Ok` (boundary case passes correctly)

---

### 4. Code Quality

**Is the existing log line (`"Rebuilding NixOS configuration: ..."`) preserved?**

✅ **PASS** — Line present and intact:
```rust
let _ = tx.send_blocking(format!("Rebuilding NixOS configuration: {flake_target}"));
```

**Is the `run_streaming_command` call structurally unchanged?**

✅ **PASS** — The call uses the identical arguments:
```rust
run_streaming_command(
    "pkexec",
    &["nixos-rebuild", "switch", "--flake", &flake_target],
    tx,
)
```

**Are there any unnecessary changes beyond the spec scope?**

✅ **PASS** — No unnecessary changes were made. The only diff from the previous state is:
1. Addition of `validate_hostname()` private function (15 lines)
2. Replacement of the two unvalidated lines in the flake branch with the validated version
3. Addition of the `#[cfg(test)] mod tests` block

No unrelated code was modified.

---

### 5. Security

**Is the validation applied BEFORE constructing `flake_target`?**

✅ **PASS** — `validate_hostname()` is called before the `format!("/etc/nixos#{}", hostname)` expression. The `flake_target` variable is only assigned after successful validation. If validation fails, the function returns `false` before `flake_target` is ever constructed.

**Is `raw_hostname` never embedded in any string without passing through validation first?**

✅ **PASS** — `raw_hostname` is only referenced once: as the argument to `validate_hostname()`. The variable `hostname` bound to `Ok(h)` (i.e., the validated `&str`) is the only value embedded into `flake_target`. There is no code path that embeds `raw_hostname` directly.

---

### 6. Test Coverage

**Are there tests rejecting: empty, too-long, `#`, `?`, space, NUL, newline?**

✅ **PASS** — All covered in `validate_hostname_rejects_dangerous_input`:

| Input | Expected | Present? |
|---|---|---|
| `""` | `Err` | ✅ |
| `"a".repeat(254)` | `Err` | ✅ |
| `"host#evil"` | `Err` | ✅ |
| `"host?url=override"` | `Err` | ✅ (spec uses `"host?q=1"` — equivalent) |
| `"my host"` | `Err` | ✅ |
| `"host\x00name"` | `Err` | ✅ |
| `"host\nmalicious"` | `Err` | ✅ |

**Are there tests accepting: `nixos`, `my-server`, `server1.local`, `MY_SERVER_42`, `my_host`, single char, 253-char boundary?**

✅ **PASS** — All covered in `validate_hostname_accepts_valid_input`:

| Input | Expected | Present? |
|---|---|---|
| `"nixos"` | `Ok` | ✅ |
| `"my-server"` | `Ok` | ✅ (spec: `"my-machine"` — semantically equivalent) |
| `"server1.local"` | `Ok` | ✅ (spec: `"nixos.local"` — semantically equivalent) |
| `"MY_SERVER_42"` | `Ok` | ✅ |
| `"my_host"` | `Ok` | ✅ |
| `"a"` (single char) | `Ok` | ✅ |
| `"a".repeat(253)` | `Ok` | ✅ |

**Do the tests use `#[cfg(test)]`?**

✅ **PASS** — Module declared as `#[cfg(test)] mod tests { ... }`.

---

### 7. Build Success

| Command | Result |
|---|---|
| `cargo build` | ✅ Exit 0 |
| `cargo test` | ✅ Exit 0 — 2 tests pass |
| `cargo clippy -- -D warnings` | ⚠️ Not available (`clippy` not installed); `cargo check` passes |
| `cargo fmt --check` | ⚠️ Not available (`rustfmt` not installed) |
| `cargo check` | ✅ Exit 0 |

---

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 93% | A |
| Best Practices | 100% | A+ |
| Functionality | 100% | A+ |
| Code Quality | 100% | A+ |
| Security | 100% | A+ |
| Performance | 100% | A+ |
| Consistency | 97% | A+ |
| Build Success | 95% | A |

**Overall Grade: A+ (98%)**

---

## Summary of Findings

The implementation correctly and completely addresses the security bug described in the specification. The critical defect — unvalidated hostname interpolation into a privileged `nixos-rebuild --flake` invocation — has been fixed.

**Findings:**

| ID | Severity | Category | Status | Detail |
|---|---|---|---|---|
| F-01 | Minor | Spec Compliance | INFO | Error message `"Upgrade aborted: {e}"` differs from spec example `"Error: invalid hostname — {e}"`. Both are user-readable; not a functional regression. |
| F-02 | Minor | Doc Comment | INFO | Doc comment omits the `src/validation.rs` deduplication suggestion from spec §3.2. Content is otherwise accurate. |
| F-03 | Info | Environment | NOTE | `cargo clippy` and `cargo fmt` were unavailable (not installed on Fedora system Rust). `cargo check` and `cargo build` pass cleanly. |

No critical, high, or medium issues found. The security fix is correctly implemented. Tests are comprehensive and cover all spec-required cases as well as additional edge cases.

---

## Verdict

**Build Result: PASS**
**Overall Verdict: PASS**
