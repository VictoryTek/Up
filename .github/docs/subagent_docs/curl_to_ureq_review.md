# Review: Replace `curl` shell-outs with `ureq` (`curl_to_ureq`)

**Backlog Item:** 10  
**Files Reviewed:** `src/upgrade/version.rs`, `Cargo.toml`  
**Spec:** `.github/docs/subagent_docs/curl_to_ureq_spec.md`  
**Review Date:** 2026-05-07  

---

## Build Validation Results

### `cargo fmt --check`

```
(no output)
fmt exit: 0
```

**Result: PASS** — Zero formatting diffs.

---

### `cargo check`

All errors are GTK/pkg-config build-script failures (expected on Windows — no GTK4 system libraries present).  
Zero lines reference `src/upgrade/version.rs` or `ureq`.  
No `error[E...]` Rust compiler errors from user source files.

```
error: failed to run custom build command for `gio-sys v0.20.10`
  -- pkg-config command could not be found (Windows, expected)
error: failed to run custom build command for `gobject-sys v0.20.10`
  -- same Windows/pkg-config expected failure
(further GTK-family crate failures follow the same pattern)
```

**Result: PASS** — No ureq-related or `version.rs`-related Rust errors.

---

## Code Review Findings

### ✅ Passing Items

| Item | Status |
|------|--------|
| `ureq = "3"` added to `Cargo.toml` | ✓ |
| All 4 `curl` call sites replaced with `ureq` | ✓ |
| `use std::io::Read` present (required for `.read_to_string`) | ✓ |
| `use std::process::Command` retained (needed for `do-release-upgrade` fallback) | ✓ |
| Timeout configured on every agent (`Duration::from_secs(15)`) | ✓ |
| Ubuntu body fetch: `.into_body().read_to_string(&mut body)` — valid ureq 3.x pattern | ✓ |
| Thread safety: all calls remain on `std::thread::spawn` background threads | ✓ |
| URLs are hardcoded strings — no user-controlled injection risk | ✓ |
| ureq 2.x patterns absent (no `.into_string()`, no `ureq::Error::Status`) | ✓ |
| `cargo fmt --check` exit 0 | ✓ |

---

### ❌ Issues Found

---

#### CRITICAL — C1: Network errors misreported as "not released" in all three status-code probe functions

**Affected functions:** `check_fedora_upgrade`, `check_opensuse_upgrade`, `check_nixos_upgrade`

**Current implementation:**
```rust
match agent.get(&url).call() {
    Ok(_)  => format!("Yes — Fedora {} is available", next),
    Err(_) => format!("No — Fedora {} not yet released", next),
}
```

**Spec-required pattern:**
```rust
fn url_exists(agent: &ureq::Agent, url: &str) -> Result<bool, ureq::Error> {
    match agent.get(url).call() {
        Ok(_)                           => Ok(true),
        Err(ureq::Error::StatusCode(_)) => Ok(false),  // 404 = genuinely not available
        Err(e)                          => Err(e),      // timeout/DNS = propagate
    }
}

// Callers:
match url_exists(&agent, &url) {
    Ok(true)  => format!("Yes — Fedora {} is available", next),
    Ok(false) => format!("No — Fedora {} not yet released", next),
    Err(e)    => format!("Could not check: {e}"),
}
```

**Impact:** When the user's network times out, DNS fails, or the remote server is unreachable, the application displays `"No — Fedora X not yet released"` — a factually incorrect statement that misleads the user into believing the next distribution version has not been released yet, when in fact the check simply could not complete. This is a correctness and UX failure.

The spec explicitly defined `ureq::Error::StatusCode(_)` as the "not available" branch and `Err(e)` as the "could not check" branch (spec sections 3.2 and 4.4–4.6).

**Required fix:** Implement the `url_exists()` helper (or inline equivalent) that matches `ureq::Error::StatusCode(_)` separately from network/IO errors.

---

#### RECOMMENDED — R1: No shared `http_agent()` helper — agent construction duplicated four times

**Spec section 4.1** specified a single `http_agent()` private helper to be called by all four check functions:

```rust
fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .into()
}
```

**Current implementation** copies the identical three-line agent builder block inline in `fetch_ubuntu_meta_release`, `check_fedora_upgrade`, `check_opensuse_upgrade`, and `check_nixos_upgrade`.

**Impact:** Minor — the code works, but any future change to the timeout or agent configuration must be updated in four places instead of one. Violates DRY and spec-prescribed architecture.

---

#### RECOMMENDED — R2: Timeout value is 15 s instead of spec-required 10 s

**Spec section 4.1:** `timeout_global(Some(std::time::Duration::from_secs(10)))`  
**Implementation:** `timeout_global(Some(Duration::from_secs(15)))`

The spec derived 10 s from the original `curl --max-time 10` flag in the Ubuntu fetch. Using 15 s is not wrong, but it deviates from the specification and makes the app less responsive when a server is genuinely unreachable.

---

#### INFO — I1: `into_body()` + `std::io::Read::read_to_string` vs spec's `body_mut().read_to_string()`

**Spec pattern (section 4.3):**
```rust
.body_mut()
.read_to_string()    // ureq Body's own method — returns Result<String>
```

**Implementation:**
```rust
.into_body()
.read_to_string(&mut body)    // std::io::Read trait method — returns Result<usize>
```

Both are valid ureq 3.x patterns. The spec's `body_mut().read_to_string()` (no argument) is slightly simpler. The implementation's `into_body()` + `Read::read_to_string(&mut buf)` is functionally correct and idiomatic Rust. Not a defect.

---

#### INFO — I2: `new_agent()` vs `.into()` for agent construction

**Spec:** `.build().into()`  
**Implementation:** `.build().new_agent()`

Both convert `AgentConfig` into `Agent`. `new_agent()` is more explicit. Not a defect.

---

## Score Table

| Category | Score | Grade |
|---|---|---|
| Specification Compliance | 72% | C |
| Best Practices | 80% | B |
| Functionality | 70% | C |
| Code Quality | 80% | B |
| Security | 100% | A |
| Performance | 90% | A |
| Consistency | 78% | C |
| Build Success | 100% | A |

**Overall Grade: C+ (84%)**

_Deductions: Specification Compliance and Functionality reduced by the CRITICAL error-discrimination gap (C1). Consistency and Code Quality reduced by duplicated agent construction (R1) and timeout deviation (R2)._

---

## Summary

**`cargo fmt --check`:** Exit 0 — no formatting issues.  
**`cargo check`:** All errors are GTK/pkg-config Windows-environment failures. Zero ureq or `version.rs` Rust compiler errors.  
**Build result (Rust code):** PASS — code compiles correctly on Linux.

**CRITICAL issue count:** 1  
**RECOMMENDED issue count:** 2  
**INFO count:** 2

The implementation successfully replaces all four `curl` call sites and the ureq 3.x API is used correctly with no deprecated 2.x patterns. However, the three status-code probe functions (Fedora, openSUSE, NixOS) use an undifferentiated `Err(_)` catch-all that conflates genuine HTTP 404 responses with network failures. This causes the application to display false "not yet released" messages when the user's network is unreachable or the server times out, violating the specification's explicit error-discrimination requirement.

---

## Verdict: NEEDS_REFINEMENT

**Required actions before approval:**

1. **(CRITICAL — C1)** Implement `url_exists()` helper (or inline equivalent) that matches `ureq::Error::StatusCode(_)` for "not available" and returns a distinct `"Could not check: {e}"` message for network/IO errors in all three probe functions.
2. **(RECOMMENDED — R1)** Extract shared `http_agent()` helper to eliminate duplicated agent construction.
3. **(RECOMMENDED — R2)** Change timeout to 10 s to match spec.
