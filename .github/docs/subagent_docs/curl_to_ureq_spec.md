# Specification: Replace `curl` shell-outs with `ureq`

**Feature name:** `curl_to_ureq`  
**Backlog item:** 10  
**File(s) modified:** `Cargo.toml`, `src/upgrade/version.rs`  
**Status:** Ready for Implementation

---

## 1. Current State Analysis

### 1.1 All `curl` call sites

All four `curl` invocations are in **`src/upgrade/version.rs`**. No other source file spawns `curl` (the `homebrew.rs` match is only a test fixture string, not a subprocess call).

---

#### Call site 1 — `fetch_ubuntu_meta_release()` (lines ~56–71)

```rust
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
```

| Property | Value |
|---|---|
| URL | `https://changelogs.ubuntu.com/meta-release` OR `https://changelogs.ubuntu.com/meta-release-lts` |
| Purpose | Fetches complete body text of Ubuntu meta-release file |
| Timeout | 10 s (`--max-time`) |
| Error handling | Maps spawn error and non-zero exit to `Err(String)` |
| Return type | `Result<String, String>` |
| Body needed? | **Yes** — full text body parsed by `parse_meta_release_for_upgrade()` |

---

#### Call site 2 — `check_fedora_upgrade()` (lines ~189–212)

```rust
match Command::new("curl")
    .args(["-s", "-o", "/dev/null", "-w", "%{http_code}", &url])
    .output()
{
    Ok(output) => {
        let code = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if code == "200" || code == "301" || code == "302" { ... }
    }
    Err(_) => "Could not check (curl not found)".to_string(),
}
```

| Property | Value |
|---|---|
| URL | `https://dl.fedoraproject.org/pub/fedora/linux/releases/{next}/Everything/x86_64/os/` |
| Purpose | HTTP status-code probe — does the next Fedora release directory exist? |
| Timeout | **None** (no `--max-time` flag) |
| Error handling | `Err(_)` → fallback string; checks raw HTTP code string |
| Body needed? | No — only status code matters |

---

#### Call site 3 — `check_opensuse_upgrade()` (lines ~234–261)

Identical pattern to call site 2.

| Property | Value |
|---|---|
| URL | `https://download.opensuse.org/distribution/leap/{next_version}/repo/oss/` |
| Purpose | HTTP status-code probe for next openSUSE Leap release |
| Timeout | **None** |
| Error handling | Same as Fedora |
| Body needed? | No |

---

#### Call site 4 — `check_nixos_upgrade()` (lines ~317–337)

Identical pattern to call sites 2 and 3.

| Property | Value |
|---|---|
| URL | `https://channels.nixos.org/{next_channel}` |
| Purpose | HTTP status-code probe for next NixOS stable channel |
| Timeout | **None** |
| Error handling | Same as Fedora |
| Body needed? | No |

---

### 1.2 Threading context

`check_upgrade_available()` is always called inside `std::thread::spawn` (see `src/ui/upgrade_page.rs` line ~414). All four inner check functions are therefore already running on a background OS thread. **ureq's synchronous API is a direct fit — no `spawn_blocking` or additional threading required.**

---

## 2. Problem Definition

- Runtime dependency on the `curl` binary: not available on all minimal Linux installs; produces unhelpful error messages when absent.
- No timeout on the status-code probes (Fedora, openSUSE, NixOS), risking infinite hangs if a server is slow.
- Process-spawn overhead on every upgrade-availability check.
- Error messages are coarse ("curl not found" or exit-code strings).

---

## 3. `ureq` Research (Context7-verified)

### 3.1 Library identity

| Property | Value |
|---|---|
| Crate | `ureq` |
| crates.io | https://crates.io/crates/ureq |
| Context7 ID | `/websites/rs_ureq` |
| Recommended version | `"3"` (ureq 3.x, stable as of 2025–2026) |
| TLS default | **rustls** — pure-Rust, no system OpenSSL or curl dependency |
| Pure Rust? | **Yes** — no C FFI by default |

### 3.2 Key APIs (ureq 3.x)

**Creating an Agent with timeout:**

```rust
use ureq::Agent;
use std::time::Duration;

let agent: Agent = Agent::config_builder()
    .timeout_global(Some(Duration::from_secs(10)))
    .build()
    .into();
```

**Making a GET request and reading body:**

```rust
let body: String = agent
    .get("https://changelogs.ubuntu.com/meta-release-lts")
    .call()?
    .body_mut()
    .read_to_string()?;
```

**HTTP status-code probe (availability check):**

```rust
// ureq 3.x follows 301/302 redirects automatically.
// Returns Ok(response) for 2xx; Err(Error::StatusCode(code)) for 4xx/5xx.
// A successful call() — regardless of final redirect chain — means the URL is reachable.
match agent.get(&url).call() {
    Ok(_) => format!("Yes — Fedora {} is available", next),
    Err(ureq::Error::StatusCode(code)) => {
        format!("No — Fedora {} not yet released (HTTP {})", next, code)
    }
    Err(e) => format!("Could not check: {e}"),
}
```

**Error type variants:**

```rust
pub enum ureq::Error {
    StatusCode(u16),          // HTTP 4xx or 5xx
    Http(HttpError),          // invalid URI, etc.
    Io(IoError),              // network/timeout I/O error
    ConnectProxyFailed(String),
    // + others
}
```

### 3.3 Cargo.toml entry

```toml
ureq = "3"
```

No extra feature flags required. The default feature set includes:
- `rustls` — pure-Rust TLS (enabled by default)
- `rustls-native-certs` — loads system CA certificate store (enabled by default)
- Redirect following — enabled by default (max 10 redirects)
- No `native-tls` needed

---

## 4. Architecture Design

### 4.1 Shared Agent helper

Create one private helper function at the top of `version.rs` that builds a shared `Agent` with a uniform 10-second global timeout. All four check functions call this helper. The agent is inexpensive to construct and is not `'static`, so we create it once per top-level check call rather than as a `lazy_static` / `OnceLock`.

```rust
fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .into()
}
```

**Rationale:** Each top-level call (`check_ubuntu_upgrade`, `check_fedora_upgrade`, etc.) is short-lived and single-threaded. A per-call agent is idiomatic and avoids any `Send` concerns.

### 4.2 Status-code probe helper

The Fedora, openSUSE, and NixOS checks all follow the same pattern. Extract to a private helper:

```rust
/// Returns true when the URL responds with any 2xx or redirect-resolved 2xx status.
/// ureq follows redirects automatically, so a successful call() means reachable.
fn url_exists(agent: &ureq::Agent, url: &str) -> Result<bool, ureq::Error> {
    match agent.get(url).call() {
        Ok(_) => Ok(true),
        Err(ureq::Error::StatusCode(_)) => Ok(false),
        Err(e) => Err(e),
    }
}
```

### 4.3 Ubuntu meta-release fetch replacement

```rust
fn fetch_ubuntu_meta_release(policy: &str) -> Result<String, String> {
    let url = match policy {
        "normal" => "https://changelogs.ubuntu.com/meta-release",
        _ => "https://changelogs.ubuntu.com/meta-release-lts",
    };
    let agent = http_agent();
    agent
        .get(url)
        .call()
        .map_err(|e| format!("HTTP request failed: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("Failed to read response body: {e}"))
}
```

**Changes from current implementation:**
- Remove `Command::new("curl")` entirely
- Remove explicit UTF-8 conversion (ureq's `read_to_string()` handles encoding)
- Timeout is now enforced (10 s) rather than relying on `--max-time`

### 4.4 Fedora upgrade check replacement

```rust
fn check_fedora_upgrade(current_version_id: &str) -> String {
    let current: u32 = current_version_id.parse().unwrap_or(0);
    let next = current + 1;
    let url = format!(
        "https://dl.fedoraproject.org/pub/fedora/linux/releases/{}/Everything/x86_64/os/",
        next
    );
    let agent = http_agent();
    match url_exists(&agent, &url) {
        Ok(true)  => format!("Yes — Fedora {} is available", next),
        Ok(false) => format!("No — Fedora {} not yet released", next),
        Err(e)    => format!("Could not check: {e}"),
    }
}
```

### 4.5 openSUSE upgrade check replacement

```rust
fn check_opensuse_upgrade(version_id: &str) -> String {
    let Some(next_version) = next_opensuse_leap_version(version_id) else {
        return "Could not parse current openSUSE Leap version".to_string();
    };
    let url = format!(
        "https://download.opensuse.org/distribution/leap/{}/repo/oss/",
        next_version
    );
    let agent = http_agent();
    match url_exists(&agent, &url) {
        Ok(true)  => format!("Yes \u{2014} openSUSE Leap {} is available", next_version),
        Ok(false) => format!("No \u{2014} openSUSE Leap {} not yet released", next_version),
        Err(e)    => format!("Could not check: {e}"),
    }
}
```

### 4.6 NixOS upgrade check replacement

```rust
fn check_nixos_upgrade(current_version_id: &str) -> String {
    let Some(next_channel) = next_nixos_channel(current_version_id) else {
        return "Could not parse current NixOS version".to_string();
    };
    let version_label = next_channel.trim_start_matches("nixos-");
    let url = format!("https://channels.nixos.org/{}", next_channel);
    let agent = http_agent();
    match url_exists(&agent, &url) {
        Ok(true)  => format!("Yes — NixOS {} is available", version_label),
        Ok(false) => format!("No — NixOS {} not yet available", version_label),
        Err(e)    => format!("Could not check: {e}"),
    }
}
```

---

## 5. Files to Modify

### 5.1 `Cargo.toml`

Add one line in the `[dependencies]` section:

```toml
ureq = "3"
```

No other dependency changes. `ureq` has no overlap with existing deps (`tokio`, `glib`, `async-channel`).

### 5.2 `src/upgrade/version.rs`

Changes required:

1. **Remove** the `use std::process::Command;` import (if it becomes unused — verify no other `Command` usages remain in the file after the change; the file uses it only for curl calls).
2. **Add** `use std::time::Duration;` (needed for `timeout_global`).
3. **Add** `http_agent()` private function.
4. **Add** `url_exists()` private function.
5. **Replace** `fetch_ubuntu_meta_release()` body — remove `Command::new("curl")`, use `http_agent()`.
6. **Replace** `check_fedora_upgrade()` body — remove `Command::new("curl")`, use `url_exists()`.
7. **Replace** `check_opensuse_upgrade()` body — remove `Command::new("curl")`, use `url_exists()`.
8. **Replace** `check_nixos_upgrade()` body — remove `Command::new("curl")`, use `url_exists()`.
9. **Update** `UbuntuUpgradeInfo::CheckFailed` doc comment: remove "missing curl" from description.

**Verify:** After the changes, `std::process::Command` should no longer be imported by `version.rs`. The import line `use std::process::Command;` at the top of `version.rs` must be removed.

---

## 6. Implementation Steps

1. **`Cargo.toml`** — add `ureq = "3"` to `[dependencies]`.
2. **`src/upgrade/version.rs`**:
   a. Remove `use std::process::Command;`
   b. Add `use std::time::Duration;`
   c. Add `fn http_agent() -> ureq::Agent` (see §4.1)
   d. Add `fn url_exists(agent: &ureq::Agent, url: &str) -> Result<bool, ureq::Error>` (see §4.2)
   e. Replace `fetch_ubuntu_meta_release()` (see §4.3)
   f. Replace `check_fedora_upgrade()` (see §4.4)
   g. Replace `check_opensuse_upgrade()` (see §4.5)
   h. Replace `check_nixos_upgrade()` (see §4.6)
   i. Update `UbuntuUpgradeInfo::CheckFailed` doc comment to say "network error, parse error" (drop "missing curl")
3. **Run `cargo build`** — verify no compilation errors.
4. **Run `cargo clippy -- -D warnings`** — verify no warnings.
5. **Run `cargo fmt --check`** — verify formatting.
6. **Run `cargo test`** — existing unit tests in `version.rs` test pure parsing logic (`next_nixos_channel`, `next_opensuse_leap_version`, `validate_hostname`, `parse_df_avail_bytes`) — all should continue passing.

---

## 7. Error Handling Comparison

| Scenario | Current (curl) | Proposed (ureq) |
|---|---|---|
| `curl` binary not found | `Err("curl not found: No such file...")` | N/A — no subprocess |
| Network unreachable / timeout | `Err("curl exited with code N")` | `Err(ureq::Error::Io(...))` → `format!("Could not check: {e}")` |
| HTTP 404 (release not yet out) | `code == "404"` → "not yet released" | `Err(Error::StatusCode(404))` → `Ok(false)` via `url_exists()` |
| HTTP 200 (release exists) | `code == "200"` → available | `Ok(_)` → `Ok(true)` |
| HTTP 301/302 (redirect) | `code == "301"` or `"302"` → available | ureq follows redirect → final 200 → `Ok(true)` |
| Bad UTF-8 in meta-release | `Err("not valid UTF-8: ...")` | ureq `read_to_string()` replaces invalid chars with `?` (lossy) |

**Improvement:** The ureq path eliminates two failure modes (curl not installed, curl version differences) and gives uniform timeout enforcement on all four call sites.

---

## 8. Risks and Mitigations

### 8.1 SSL certificate handling on NixOS

**Risk:** ureq 3.x with rustls uses `rustls-native-certs` to load the system CA bundle. On NixOS, the system certificate store may not be at the standard `/etc/ssl/certs/ca-certificates.crt` path. If `SSL_CERT_FILE` is not set, the TLS handshake to HTTPS endpoints may fail.

**Mitigation:** NixOS sets `SSL_CERT_FILE` in the default environment (via `security.pki.certificateFiles` or `cacert` package). The Flatpak sandbox inherits host environment variables. This has not been a reported issue for ureq-based Rust binaries on NixOS. If it becomes an issue, the Nix expression can set `SSL_CERT_FILE` explicitly. No code change needed now.

**Alternative (if needed):** Add the `native-tls` feature to `ureq` and use the system OpenSSL. However, this reintroduces a system library dependency. Prefer the rustls default and monitor.

### 8.2 Blocking ureq in async context

**Risk:** ureq is synchronous. If someone calls these functions from an async context directly (e.g., inside `tokio::spawn`), it would block the async runtime thread.

**Mitigation:** All four functions are currently called inside `std::thread::spawn` (confirmed at `upgrade_page.rs` line ~414). The blocking is intentional and already isolated. No change needed. The spec notes this architectural constraint for future maintainers.

### 8.3 ureq redirect behaviour difference

**Risk:** The current curl check explicitly treats HTTP 301/302 as "available" by checking the raw status code. ureq follows redirects automatically, so the caller sees the final response (200 or 404). This is _more_ correct behaviour but changes the observable path.

**Mitigation:** The functional outcome is identical: if the release directory URL redirects to a live page, it is available. If it 404s after following redirects, it is not. This is a behaviour improvement, not a regression.

### 8.4 Compile-time addition of `ureq` dependency

**Risk:** Adding a new crate increases compile time and binary size.

**Mitigation:** ureq 3.x is a minimal, low-dependency crate (no tokio, no reqwest, no async runtime). Its transitive dependency tree is small. Binary size increase is expected to be < 1 MB. This is an acceptable trade-off for eliminating the curl runtime dependency.

---

## 9. Dependencies

| Crate | Version | Features | Reason |
|---|---|---|---|
| `ureq` | `"3"` | (defaults) | Pure-Rust HTTPS client with rustls TLS |

No other dependency changes.

---

## 10. Summary

- **4 curl call sites** — all in `src/upgrade/version.rs`
  - 1 fetches a full response body (Ubuntu meta-release)
  - 3 perform HTTP status-code probes (Fedora, openSUSE Leap, NixOS channels)
- **ureq version:** `"3"` (ureq 3.x, stable, pure-Rust rustls TLS by default)
- **Files to modify:** `Cargo.toml` (+1 line), `src/upgrade/version.rs` (replace 4 functions, add 2 helpers, clean imports)
- **No other files** need to change
- All existing unit tests cover pure parsing logic and will continue to pass

Spec saved at: `c:\Projects\Up\.github\docs\subagent_docs\curl_to_ureq_spec.md`
