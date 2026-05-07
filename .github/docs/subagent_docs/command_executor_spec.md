# Specification: Backlog Item 5 — `CommandExecutor` Trait + `MockExecutor` + Parser Unit Tests

**Date:** 2026-05-07  
**Author:** Research & Specification Subagent  
**Feature name:** `command_executor`

---

## 1. Current State Analysis

### 1.1 Command Execution Architecture

The codebase has one concrete command-running type: `CommandRunner` (in `src/runner.rs`). It is a
struct — not a trait — and is passed by concrete reference to `Backend::run_update`:

```rust
// src/backends/mod.rs (current)
fn run_update<'a>(
    &'a self,
    runner: &'a CommandRunner,
) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>>;
```

`CommandRunner::run` is an `async fn` that:
- Logs the command as a `BackendEvent::LogLine`
- Routes `"pkexec"` calls through the long-lived `PrivilegedShell` (if present)
- Otherwise spawns `tokio::process::Command`, drains stdout+stderr concurrently, and returns the
  combined output string

There is **no trait abstraction** for command execution — `CommandRunner` cannot be swapped for a
test double.

### 1.2 The `list_available` Gap

`list_available` in all backends calls `tokio::process::Command` **directly** — completely
bypassing `CommandRunner`:

```rust
// Current pattern in every backend (e.g. os_package_manager.rs AptBackend)
fn list_available(&self) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
    Box::pin(async move {
        let out = tokio::process::Command::new("apt")
            .args(["list", "--upgradable"])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        // ...
    })
}
```

This means `list_available` cannot be tested without a real system binary.

### 1.3 Parser Functions — Already Extracted

All text parsers have already been extracted as `pub(crate)` standalone functions and already have
`#[cfg(test)]` unit tests:

| Backend | Parser functions | Tests |
|---------|-----------------|-------|
| `os_package_manager.rs` | `parse_apt_list_upgradable`, `count_apt_upgraded`, `parse_dnf_list_upgrades`, `count_dnf_upgraded`, `parse_checkupdates`, `count_pacman_upgraded`, `parse_zypper_list_updates`, `count_zypper_upgraded` | ✅ All present |
| `flatpak.rs` | `parse_flatpak_updates`, `parse_flatpak_app_line` | ✅ All present |
| `homebrew.rs` | `parse_brew_outdated`, `count_homebrew_upgraded` | ✅ All present |
| `nix.rs` | `count_nix_store_operations`, `compare_lock_nodes`, `upgrade_available_in_output`, `count_determinate_upgraded` | ✅ All present |

**What is missing** is test coverage for the **`run_update` logic** (which branching, error
handling, and result mapping happen inside the `Box::pin(async move { ... })` body). These cannot
be exercised without a `MockExecutor`.

### 1.4 Existing Dependencies Relevant to This Work

- `thiserror = "2"` — already present; `BackendError` already uses it
- `mockall` — **not present**; will be evaluated below
- `async-channel = "2"` — already present; used for `BackendEvent` streaming
- `tokio = { version = "1", features = [...] }` — already present

---

## 2. Problem Definition

Without a trait-based abstraction over command execution:

1. **`Backend::run_update` is untestable** — it receives `&CommandRunner` (a concrete struct tied to
   Tokio process spawning). No test can inject a controlled response.
2. **`list_available` is untestable** — it spawns real processes; failures or update counts cannot
   be verified without the actual package managers installed.
3. **`run_update` business logic has zero test coverage** — parsing + branching inside each
   backend's `run_update` body (e.g. APT's `count_apt_upgraded`, Pacman's `count_pacman_upgraded`,
   Nix's `is_nixos_flake()` branch selection) cannot be asserted.

---

## 3. Proposed Solution Architecture

### 3.1 `CommandExecutor` Trait

#### Location

New file: `src/executor.rs`

#### Design Constraints

Rust's dyn-compatibility rules (formerly "object safety") prohibit `async fn` in a trait used as
`dyn Trait`. Specifically, the Rust reference states: *"Not be an `async fn` (which has a hidden
`Future` type)"* is required for dispatchable methods. Therefore we cannot write:

```rust
// ❌ NOT dyn-compatible — hidden Future impl type
trait CommandExecutor {
    async fn run(&self, program: &str, args: &[&str]) -> Result<String, BackendError>;
}
```

The codebase already solves this for `Backend::run_update` by using the `Pin<Box<dyn Future>>`
pattern. We follow the same idiom for `CommandExecutor::run`:

```rust
// ✅ Dyn-compatible
pub trait CommandExecutor: Send + Sync {
    fn run<'a>(
        &'a self,
        program: &'a str,
        args: &'a [&'a str],
    ) -> Pin<Box<dyn Future<Output = Result<String, BackendError>> + Send + 'a>>;
}
```

This is dyn-compatible because:
- The method is a method (takes `&self`)
- No type parameters on the method itself
- Return type is `Pin<Box<dyn Future...>>`, not `impl Future` or `async fn`
- `Send + Sync` bounds on the trait are compatible

The trait can then be used as `&dyn CommandExecutor` everywhere.

#### Full Trait Definition

```rust
// src/executor.rs

use crate::backends::BackendError;
use std::future::Future;
use std::pin::Pin;

/// Abstracts the execution of external system commands, enabling dependency injection
/// and test doubles.
///
/// Implementations must be `Send + Sync` so they can be shared across async boundaries.
pub trait CommandExecutor: Send + Sync {
    /// Run `program` with `args`, stream output line-by-line (via internal channel),
    /// and return the full combined output on success.
    ///
    /// Returns `Err(BackendError)` on non-zero exit, spawn failure, or auth cancellation.
    fn run<'a>(
        &'a self,
        program: &'a str,
        args: &'a [&'a str],
    ) -> Pin<Box<dyn Future<Output = Result<String, BackendError>> + Send + 'a>>;
}
```

### 3.2 `CommandRunner` Implements `CommandExecutor`

`CommandRunner` already has an `async fn run(&self, program: &str, args: &[&str]) -> Result<String, BackendError>`. We add the trait `impl` by delegating to it:

```rust
// src/runner.rs (addition)

impl CommandExecutor for CommandRunner {
    fn run<'a>(
        &'a self,
        program: &'a str,
        args: &'a [&'a str],
    ) -> Pin<Box<dyn Future<Output = Result<String, BackendError>> + Send + 'a>> {
        Box::pin(self.run(program, args))
    }
}
```

> **Note:** The existing `CommandRunner::run` is `async fn run` — calling `self.run(...)` inside
> `Box::pin(...)` correctly captures the future. No body duplication is needed.

### 3.3 `Backend` Trait — Updated `run_update` Signature

Change `runner: &'a CommandRunner` to `runner: &'a dyn CommandExecutor`:

```rust
// src/backends/mod.rs (updated)

fn run_update<'a>(
    &'a self,
    runner: &'a dyn CommandExecutor,
) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>>;
```

All four backend files (`os_package_manager.rs`, `flatpak.rs`, `homebrew.rs`, `nix.rs`) implement
`Backend` and must update their `run_update` signature to match. The call sites inside the
`Box::pin(async move { ... })` body — `runner.run(...)` — require no changes because `dyn
CommandExecutor` exposes the same `run` method via dynamic dispatch.

### 3.4 `orchestrator.rs` — Unchanged Call Site

The orchestrator creates `CommandRunner` and passes it to `backend.run_update(&runner)`:

```rust
// src/orchestrator.rs (current — stays the same at the call site)
let runner = CommandRunner::new(be_tx.clone(), kind, shell.clone());
let result = backend.run_update(&runner).await;
```

Since `CommandRunner` implements `CommandExecutor`, passing `&runner` (where `runner: CommandRunner`)
to a `&dyn CommandExecutor` parameter works via automatic coercion. **No changes to
`orchestrator.rs` are required.**

### 3.5 `MockExecutor` — Hand-Written Test Double

#### Choice: Hand-Written vs. `mockall`

**Decision: hand-written `MockExecutor`.**

Rationale:
- `mockall` with async traits requires `#[automock]` + `#[async_trait]` or `Pin<Box<dyn Future>>`
  return types. When using the latter (which our trait uses), `mockall`'s code generation produces
  correct but complex expectations. For a simple response-queue pattern (pre-canned outputs per
  command call) a hand-written double is simpler, more readable, and carries zero macro overhead.
- `mockall` would add a proc-macro compilation dependency (`mockall = "0.14"` +
  `mockall_derive`) — increasing build times in test mode.
- The `MockExecutor` needed here is a straightforward FIFO queue of
  `Result<String, BackendError>` responses: no argument matchers, call counts, or sequences are
  required for backend `run_update` tests.
- A hand-written double is auditable and does not depend on `mockall`'s internal expansion
  behavior, which has changed across versions.

**Future consideration:** If more complex interaction testing is needed (e.g. verifying the exact
command arguments passed), `mockall` can be added at that point. The `CommandExecutor` trait is
designed to be `#[automock]`-compatible if needed.

#### `MockExecutor` Design

```rust
// src/executor.rs (inside #[cfg(test)] or a dedicated test_utils module)

#[cfg(test)]
pub mod test_utils {
    use super::*;
    use crate::backends::BackendError;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// A test double for [`CommandExecutor`] that returns pre-configured responses
    /// in FIFO order. Each call to `run` consumes one response from the queue.
    ///
    /// Panics if `run` is called more times than responses were enqueued.
    #[derive(Clone)]
    pub struct MockExecutor {
        responses: Arc<Mutex<VecDeque<Result<String, BackendError>>>>,
    }

    impl MockExecutor {
        /// Create a `MockExecutor` pre-loaded with the given responses.
        /// The first call to `run` returns `responses[0]`, the second returns `responses[1]`, etc.
        pub fn new(responses: Vec<Result<String, BackendError>>) -> Self {
            Self {
                responses: Arc::new(Mutex::new(responses.into())),
            }
        }

        /// Convenience: create with a single successful output string.
        pub fn with_output(output: impl Into<String>) -> Self {
            Self::new(vec![Ok(output.into())])
        }

        /// Convenience: create with a single `BackendError::Exit` response.
        pub fn with_error(code: i32, message: impl Into<String>) -> Self {
            Self::new(vec![Err(BackendError::Exit {
                code,
                message: message.into(),
            })])
        }
    }

    impl CommandExecutor for MockExecutor {
        fn run<'a>(
            &'a self,
            _program: &'a str,
            _args: &'a [&'a str],
        ) -> Pin<Box<dyn Future<Output = Result<String, BackendError>> + Send + 'a>> {
            let response = self
                .responses
                .lock()
                .expect("MockExecutor mutex poisoned")
                .pop_front()
                .expect("MockExecutor: no more pre-configured responses (run() called too many times)");
            Box::pin(async move { response })
        }
    }
}
```

`Arc<Mutex<VecDeque<...>>>` is used because `run` takes `&self` (shared reference) but must
mutate the queue. The `Arc` allows `MockExecutor` to be cloned across async boundaries (needed
when tests clone the executor before passing it to backend methods).

### 3.6 `run_update` Tests Per Backend

Each backend's `#[cfg(test)] mod tests` block gains integration-style tests for `run_update`.

> **Pattern:** construct the backend struct, construct a `MockExecutor` with the fixture output
> that the real command would produce, call `run_update(&mock)`, and assert on the returned
> `UpdateResult`.

Below are the complete test sketches with fixture strings for each backend.

---

## 4. Implementation Steps (Ordered)

### Step 1 — Create `src/executor.rs`

Create the new file with:
- `pub trait CommandExecutor` (the trait)
- `#[cfg(test)] pub mod test_utils { pub struct MockExecutor ... }`

### Step 2 — Expose `executor` module in `src/main.rs`

```rust
// src/main.rs
mod executor;
```

### Step 3 — Implement `CommandExecutor` for `CommandRunner` in `src/runner.rs`

Add `use crate::executor::CommandExecutor;` and the `impl CommandExecutor for CommandRunner` block.

### Step 4 — Update `Backend` trait in `src/backends/mod.rs`

- Add `use crate::executor::CommandExecutor;`
- Change `run_update` signature: `runner: &'a CommandRunner` → `runner: &'a dyn CommandExecutor`
- Remove the `use crate::runner::CommandRunner;` import (now only `CommandRunner` is used in
  `orchestrator.rs` directly)

### Step 5 — Update All `Backend` Implementors

Update `fn run_update<'a>(&'a self, runner: &'a CommandRunner)` → `&'a dyn CommandExecutor` in:
- `src/backends/os_package_manager.rs` — four structs: `AptBackend`, `DnfBackend`,
  `PacmanBackend`, `ZypperBackend`
- `src/backends/flatpak.rs` — `FlatpakBackend`
- `src/backends/homebrew.rs` — `HomebrewBackend`
- `src/backends/nix.rs` — `NixBackend`

Also update the `use crate::runner::CommandRunner;` imports in each backend file: replace with
`use crate::executor::CommandExecutor;`.

### Step 6 — Write `run_update` Tests in Each Backend File

Add tests inside the existing `#[cfg(test)] mod tests { ... }` block in each backend file.

### Step 7 — Run `cargo test` and `cargo clippy -- -D warnings`

Verify all existing tests still pass, and the new tests compile and pass.

### Step 8 — (Optional / Future) Route `list_available` Through the Executor

This is a **separate backlog item**. The current `list_available` implementations bypass
`CommandRunner` entirely by calling `tokio::process::Command` directly. Routing them through
`CommandExecutor` would require changing the `Backend` trait's `list_available` signature to also
accept `&dyn CommandExecutor`. This is a more invasive change and is deferred.

---

## 5. Files to Create / Modify

| File | Action | Summary |
|------|--------|---------|
| `src/executor.rs` | **Create** | `CommandExecutor` trait + `MockExecutor` in `#[cfg(test)]` |
| `src/main.rs` | **Modify** | Add `mod executor;` |
| `src/runner.rs` | **Modify** | Add `impl CommandExecutor for CommandRunner` |
| `src/backends/mod.rs` | **Modify** | Update `Backend::run_update` signature; add import |
| `src/backends/os_package_manager.rs` | **Modify** | Update `run_update` signature × 4; add `run_update` tests |
| `src/backends/flatpak.rs` | **Modify** | Update `run_update` signature; add `run_update` tests |
| `src/backends/homebrew.rs` | **Modify** | Update `run_update` signature; add `run_update` tests |
| `src/backends/nix.rs` | **Modify** | Update `run_update` signature; add `run_update` tests |

---

## 6. Cargo.toml Changes

**No new runtime dependencies** are needed.

If `mockall` is chosen in the future (rejected for now — see §3.5), it would be:

```toml
[dev-dependencies]
mockall = "0.14"
```

For now, **no `[dev-dependencies]` block is needed** because `MockExecutor` is hand-written and
lives inside `#[cfg(test)]` in `src/executor.rs`.

---

## 7. Parser Inventory and Fixture Strings

All parsers already exist and are tested. The following table documents the existing parsers and
their fixture strings as a reference. The **gap** is that `run_update` tests (which exercise the
full command → parse → `UpdateResult` pipeline) are missing.

### 7.1 `os_package_manager.rs`

#### APT

**`parse_apt_list_upgradable(output: &str) -> Vec<String>`**

Fixture (`apt list --upgradable` output):
```
Listing... Done
htop/noble,now 3.3.0-1 amd64 [upgradable from: 3.2.2-1]
curl/noble,now 8.5.0-1 amd64 [upgradable from: 8.4.0-1]
libssl3/noble,now 3.3.1-2ubuntu2 amd64 [upgradable from: 3.3.0-1ubuntu2]
```
Expected result: `["htop", "curl", "libssl3"]`  
Missing test: package with epoch prefix (`1:curl/...`)

**`count_apt_upgraded(output: &str) -> usize`**

Fixture (`apt upgrade -y` summary line):
```
3 upgraded, 1 newly installed, 0 to remove and 2 not upgraded.
```
Expected result: `3`

**`run_update` test — `AptBackend`**  
Mock response for `runner.run("pkexec", ["sh", "-c", "...apt update && ...apt upgrade -y"])`:
```
Get:1 http://archive.ubuntu.com/ubuntu noble InRelease [256 kB]
...
Reading package lists...
Building dependency tree...
3 upgraded, 0 newly installed, 0 to remove and 1 not upgraded.
```
Assert: `UpdateResult::Success { updated_count: 3 }`

**`run_update` error test — `AptBackend`**  
Mock response: `Err(BackendError::AuthCancelled)`  
Assert: `UpdateResult::Error(BackendError::AuthCancelled)`

#### DNF

**`parse_dnf_list_upgrades(output: &str) -> Vec<String>`**

Fixture (`dnf check-update` output, exit 100):
```
Last metadata expiration check: 0:01:23 ago on Wed May  7 10:00:00 2025.

htop.x86_64                    3.3.0-2.fc40          updates
curl.x86_64                    8.5.0-1.fc40          updates
openssl-libs.x86_64            3.2.1-1.fc40          updates
```
Expected result: `["htop.x86_64", "curl.x86_64", "openssl-libs.x86_64"]`

**`count_dnf_upgraded(output: &str) -> usize`**

Fixture (DNF4):
```
  Upgrade  15 Packages

Transaction Summary
================================================================================
```
Expected result: `15`

Fixture (DNF5):
```
  Upgrading: 7 packages
```
Expected result: `7`

**`run_update` test — `DnfBackend`**  
Mock response for `runner.run("pkexec", ["dnf", "upgrade", "-y"])`:
```
Last metadata expiration check: 0:00:01 ago.
Dependencies resolved.
================================================================================
  Upgrading: 7 packages
================================================================================
Complete!
```
Assert: `UpdateResult::Success { updated_count: 7 }`

#### Pacman

**`parse_checkupdates(output: &str) -> Vec<String>`**

Fixture (`pacman -Qu` output):
```
htop 3.2.2-1 -> 3.3.0-1
curl 8.4.0-1 -> 8.5.0-1
linux 6.8.1.arch1-1 -> 6.8.2.arch1-1
```
Expected result: `["htop", "curl", "linux"]`

**`count_pacman_upgraded(output: &str) -> usize`**

Fixture (`pacman -Syu --noconfirm` output):
```
:: Synchronizing package databases...
 core is up to date
 extra is up to date
resolving dependencies...
upgrading htop
upgrading curl
upgrading linux
:: Running post-transaction hooks...
```
Expected result: `3`

**`run_update` test — `PacmanBackend`**  
Mock response for `runner.run("pkexec", ["pacman", "-Syu", "--noconfirm"])`:  
(use fixture above)  
Assert: `UpdateResult::Success { updated_count: 3 }`

#### Zypper

**`parse_zypper_list_updates(output: &str) -> Vec<String>`**

Fixture (`zypper list-updates` output):
```
Loading repository data...
Reading installed packages...
S  | Repository          | Name          | Current Version | Available Version | Arch
---+---------------------+---------------+-----------------+-------------------+-------
v  | openSUSE-updates    | htop          | 3.2.2-1.1       | 3.3.0-1.1         | x86_64
v  | openSUSE-updates    | curl          | 8.4.0-1.1       | 8.5.0-1.1         | x86_64
v  | openSUSE-security   | openssl       | 3.1.1-1.1       | 3.1.4-1.1         | x86_64
```
Expected result: `["htop", "curl", "openssl"]`

**`count_zypper_upgraded(output: &str) -> usize`**

Fixture (`zypper update -y` output):
```
Retrieving package htop-3.3.0.x86_64 (1/3) ...done
Retrieving package curl-8.5.0.x86_64 (2/3) ...done
Retrieving package openssl-3.1.4.x86_64 (3/3) ...done
```
Expected result: `3`

**`run_update` test — `ZypperBackend`**  
Mock 1st response for `runner.run("pkexec", ["sh", "-c", "zypper refresh && zypper update -y"])`:  
(use fixture above)  
Assert: `UpdateResult::Success { updated_count: 3 }`

### 7.2 `flatpak.rs`

**`parse_flatpak_updates(output: &str) -> Vec<String>`**

Fixture (`flatpak update --no-deploy --columns=application` output):
```
Application
org.gnome.Calculator
com.spotify.Client
org.mozilla.firefox
```
Expected result: `["org.gnome.Calculator", "com.spotify.Client", "org.mozilla.firefox"]`

**`run_update` test — `FlatpakBackend` — with updates**  
Mock response for `runner.run("flatpak", ["update", "-y"])`:
```
Looking for updates...

   ID                            Branch         Op           Remote         Download
1. org.gnome.Calculator          stable         u            flathub        1.5 MB
2. com.spotify.Client            stable         u            flathub        87.3 MB

Updating: org.gnome.Calculator/x86_64/stable from flathub
...
```
Assert: `UpdateResult::Success { updated_count: 2 }`  
(Two lines starting with a digit: `1.` and `2.`)

**`run_update` test — `FlatpakBackend` — nothing to update**  
Mock response: `"Looking for updates...\n\nNothing to do.\n"`  
Assert: `UpdateResult::Success { updated_count: 0 }`

**`run_update` test — `FlatpakBackend` — error**  
Mock response: `Err(BackendError::Exit { code: 1, message: "flatpak: error".into() })`  
Assert: `UpdateResult::Error(...)`

### 7.3 `homebrew.rs`

**`parse_brew_outdated(output: &str) -> Vec<String>`**

Fixture (`brew outdated` output):
```
htop (3.2.2) < 3.3.0
curl (8.4.0) < 8.5.0
node (20.0.0) < 22.0.0
```
Expected result: `["htop", "curl", "node"]`

Edge cases to add:
- Package with no version info: `"ffmpeg"` → `["ffmpeg"]`
- Empty output: `""` → `[]`

**`count_homebrew_upgraded(output: &str) -> usize`**

Fixture (`brew upgrade` output):
```
==> Upgrading 2 outdated packages:
htop 3.2.2 -> 3.3.0
curl 8.4.0 -> 8.5.0
==> Upgrading htop
==> Pouring htop--3.3.0.arm64_sonoma.bottle.tar.gz
==> Upgrading curl
==> Pouring curl--8.5.0.arm64_sonoma.bottle.tar.gz
```
Expected result: `4` (2 "Upgrading" + 2 "Pouring" lines, excluding the "Upgrading N outdated
packages:" line which contains "outdated packages").

**`run_update` test — `HomebrewBackend` — with upgrades**  
Mock 1st response for `runner.run("brew", ["update"])`: `Ok("Already up-to-date.\n".to_string())`  
Mock 2nd response for `runner.run("brew", ["upgrade"])`: (fixture above)  
Assert: `UpdateResult::Success { updated_count: 4 }`

**`run_update` test — `HomebrewBackend` — `brew update` fails**  
Mock 1st response: `Err(BackendError::Exit { code: 1, message: "brew update failed".into() })`  
Assert: `UpdateResult::Error(...)` (returns early without calling `brew upgrade`)

### 7.4 `nix.rs`

**`count_nix_store_operations(output: &str) -> usize`**

Fixture (combined build + fetch):
```
these 2 derivations will be built:
  /nix/store/abc123-htop-3.3.0.drv
  /nix/store/def456-curl-8.5.0.drv
these 5 paths will be fetched (12.4 MiB download, 45.3 MiB unpacked):
  /nix/store/ghi789-openssl-3.2.1
  ...
```
Expected result: `7`

**`upgrade_available_in_output(output: &str) -> bool`**

Fixture:
```
Determinate Nix v3.6.2
An upgrade is available: v3.7.0
Run `sudo determinate-nixd upgrade` to upgrade.
```
Expected result: `true`

**`count_determinate_upgraded(output: &str) -> usize`**

Fixture (success): `"Successfully upgraded determinate-nix to v3.7.0\n"` → `1`  
Fixture (nothing): `"nothing to upgrade\n"` → `0`

**`compare_lock_nodes` — changed rev**  
Already tested. Additional edge case: new input added (present in `new`, absent in `old`):
```rust
let old = json!({"nodes": {"nixpkgs": {"locked": {"rev": "abc", "lastModified": 100}}}});
let new = json!({"nodes": {
    "nixpkgs": {"locked": {"rev": "abc", "lastModified": 100}},
    "home-manager": {"locked": {"rev": "xyz", "lastModified": 200}}
}});
assert_eq!(compare_lock_nodes(&old, &new), vec!["home-manager"]);
```

**`run_update` test — `NixBackend` (nix profile, modern Nix, `count_nix_store_operations`)**  
This requires mocking the `nix_profile_upgrade_all` path. However, `NixBackend::run_update` calls
`nix_profile_upgrade_all()` which itself spawns `tokio::process::Command` directly (does NOT go
through `runner`). This is the most complex case.

The portions of `NixBackend::run_update` that DO go through `runner` are:
- NixOS flake rebuild: `runner.run("pkexec", ["env", "PATH=...", "sh", "-c", "..."])` 
- Legacy channels: `runner.run("pkexec", ["env", ..., "sh", "-c", "..."])`
- Determinate Nix: `runner.run("pkexec", [nixd_path, "upgrade"])`
- Legacy nix-env: `runner.run("nix-env", ["-u"])`

Tests for the paths that use `runner`:

**`run_update` test — NixOS flake path (requires `is_nixos()` and `is_nixos_flake()` to be true)**  
These functions check `/run/current-system` and `/etc/nixos/flake.nix` — not injectable. This path
cannot be unit tested without filesystem injection. It is marked as **deferred** in this backlog
item. A future backlog item should extract `is_nixos`, `is_nixos_flake`, `is_determinate_nix` as
injectable functions via a `SystemDetector` trait.

**`run_update` test — Determinate Nix path**  
Similarly requires `is_determinate_nix()` to return `true`. Deferred.

**`run_update` test — legacy nix-env path**  
Mock response for `runner.run("nix-env", ["-u"])`:
```
upgrading 'htop-3.2.2' to 'htop-3.3.0'
upgrading 'curl-8.4.0' to 'curl-8.5.0'
```
Assert: `UpdateResult::Success { updated_count: 2 }`  
(This path requires `use_legacy_nix_env = true`, which reads `~/.nix-profile/manifest.json`.
This is also not injectable without additional abstraction. Deferred.)

> **Honest Assessment for Nix:** Because `NixBackend::run_update` branches on several
> side-effecting filesystem checks (`is_nixos()`, `is_nixos_flake()`, `is_determinate_nix()`,
> manifest file reading) that are NOT injectable, the only `run_update` paths we can unit test are
> those reachable when none of those checks return `true` — i.e., the standard `nix profile
> upgrade` path. A comprehensive Nix test suite requires a second trait (`SystemProber`) in a
> future backlog item.

---

## 8. Migration Plan

### Phase A: Extract Parsers (Already Done)

All parsers are `pub(crate)` functions with `#[cfg(test)]` tests. ✅ No action needed.

### Phase B: `CommandExecutor` Trait (This Backlog Item)

1. Create `src/executor.rs` with trait + `MockExecutor`
2. Add `mod executor;` to `src/main.rs`
3. Implement `CommandExecutor for CommandRunner`
4. Update `Backend::run_update` signature in `mod.rs`
5. Update all backend `run_update` implementations
6. Add `run_update` tests using `MockExecutor`
7. Verify `cargo build`, `cargo clippy -- -D warnings`, `cargo test`

### Phase C: `list_available` Abstraction (Future Item)

Change `Backend::list_available` to accept `&dyn CommandExecutor` and update all backends. This
is a breaking change to the `Backend` trait's default implementation and all overrides.

### Phase D: `SystemProber` Abstraction (Future Item)

Extract `is_nixos()`, `is_nixos_flake()`, `is_determinate_nix()`, `is_running_in_flatpak()` into
an injectable `trait SystemProber` to unlock full `NixBackend` and `FlatpakBackend` testing.

---

## 9. Sample Code Sketches

### `src/executor.rs` (complete)

```rust
use crate::backends::BackendError;
use std::future::Future;
use std::pin::Pin;

/// Abstracts the execution of external system commands.
///
/// Using `dyn CommandExecutor` allows test doubles (`MockExecutor`) to be
/// injected in place of the real `CommandRunner` without spawning any processes.
pub trait CommandExecutor: Send + Sync {
    fn run<'a>(
        &'a self,
        program: &'a str,
        args: &'a [&'a str],
    ) -> Pin<Box<dyn Future<Output = Result<String, BackendError>> + Send + 'a>>;
}

#[cfg(test)]
pub mod test_utils {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// Pre-configured response queue for testing backends.
    #[derive(Clone)]
    pub struct MockExecutor {
        responses: Arc<Mutex<VecDeque<Result<String, BackendError>>>>,
    }

    impl MockExecutor {
        pub fn new(responses: Vec<Result<String, BackendError>>) -> Self {
            Self {
                responses: Arc::new(Mutex::new(responses.into())),
            }
        }

        pub fn with_output(output: impl Into<String>) -> Self {
            Self::new(vec![Ok(output.into())])
        }

        pub fn with_error(code: i32, message: impl Into<String>) -> Self {
            Self::new(vec![Err(BackendError::Exit {
                code,
                message: message.into(),
            })])
        }

        pub fn with_auth_cancelled() -> Self {
            Self::new(vec![Err(BackendError::AuthCancelled)])
        }
    }

    impl CommandExecutor for MockExecutor {
        fn run<'a>(
            &'a self,
            _program: &'a str,
            _args: &'a [&'a str],
        ) -> Pin<Box<dyn Future<Output = Result<String, BackendError>> + Send + 'a>> {
            let response = self
                .responses
                .lock()
                .expect("MockExecutor mutex poisoned")
                .pop_front()
                .expect("MockExecutor exhausted: run() called more times than responses");
            Box::pin(async move { response })
        }
    }
}
```

### `src/runner.rs` addition

```rust
use crate::executor::CommandExecutor;

impl CommandExecutor for CommandRunner {
    fn run<'a>(
        &'a self,
        program: &'a str,
        args: &'a [&'a str],
    ) -> Pin<Box<dyn Future<Output = Result<String, BackendError>> + Send + 'a>> {
        Box::pin(self.run(program, args))
    }
}
```

### `src/backends/os_package_manager.rs` — `AptBackend` test example

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::test_utils::MockExecutor;

    // ... existing parser tests ...

    #[tokio::test]
    async fn test_apt_run_update_success() {
        let fixture = "\
Reading package lists... Done\n\
Building dependency tree... Done\n\
3 upgraded, 0 newly installed, 0 to remove and 1 not upgraded.\n";
        let mock = MockExecutor::with_output(fixture);
        let result = AptBackend.run_update(&mock).await;
        assert!(matches!(
            result,
            UpdateResult::Success { updated_count: 3 }
        ));
    }

    #[tokio::test]
    async fn test_apt_run_update_auth_cancelled() {
        let mock = MockExecutor::with_auth_cancelled();
        let result = AptBackend.run_update(&mock).await;
        assert!(matches!(result, UpdateResult::Error(BackendError::AuthCancelled)));
    }

    #[tokio::test]
    async fn test_apt_run_update_zero_upgraded() {
        let fixture = "0 upgraded, 0 newly installed, 0 to remove and 0 not upgraded.\n";
        let mock = MockExecutor::with_output(fixture);
        let result = AptBackend.run_update(&mock).await;
        assert!(matches!(
            result,
            UpdateResult::Success { updated_count: 0 }
        ));
    }
}
```

### `src/backends/homebrew.rs` — `HomebrewBackend` test example

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::test_utils::MockExecutor;
    use crate::backends::BackendError;

    // ... existing parser tests ...

    #[tokio::test]
    async fn test_homebrew_run_update_success() {
        // brew update succeeds, brew upgrade shows 2 upgrades
        let brew_update_output = "Already up-to-date.\n";
        let brew_upgrade_output = "\
==> Upgrading 2 outdated packages:\n\
==> Upgrading htop\n\
==> Pouring htop--3.3.0.arm64_sonoma.bottle.tar.gz\n\
==> Upgrading curl\n\
==> Pouring curl--8.5.0.arm64_sonoma.bottle.tar.gz\n";
        let mock = MockExecutor::new(vec![
            Ok(brew_update_output.to_string()),
            Ok(brew_upgrade_output.to_string()),
        ]);
        let result = HomebrewBackend.run_update(&mock).await;
        assert!(matches!(
            result,
            UpdateResult::Success { updated_count: 4 }
        ));
    }

    #[tokio::test]
    async fn test_homebrew_run_update_brew_update_fails() {
        let mock = MockExecutor::new(vec![Err(BackendError::Exit {
            code: 1,
            message: "brew update error".to_string(),
        })]);
        let result = HomebrewBackend.run_update(&mock).await;
        assert!(matches!(result, UpdateResult::Error(_)));
    }
}
```

---

## 10. Risks and Mitigations

### Risk 1: Dyn-Incompatibility if Trait Signature Changes

**Risk:** If a future contributor adds a generic type parameter or `async fn` to `CommandExecutor`,
it breaks `dyn CommandExecutor` usage.

**Mitigation:** The trait is designed to be minimal — one method. Add a compile-time smoke test:
```rust
// In executor.rs, outside #[cfg(test)], inside a type-check-only block:
fn _assert_dyn_compat(_: &dyn CommandExecutor) {}
```
This ensures the compiler validates dyn-compatibility on every build.

### Risk 2: GTK Main-Loop Threading Constraints

**Risk:** GTK objects must only be accessed from the GTK main thread. `CommandExecutor` is
`Send + Sync` — tests that touch GTK objects while running in a Tokio runtime would deadlock.

**Mitigation:** `run_update` tests use `MockExecutor` and make no GTK calls. The `UpdateResult`
enum is pure data (`Send + Sync`). Tests use `#[tokio::test]` (single-threaded runtime) and never
interact with GTK. No risk in practice.

### Risk 3: `run_update` Tests for `NixBackend` — Filesystem Probing Not Injectable

**Risk:** `NixBackend::run_update` calls `is_nixos()`, `is_nixos_flake()`, `is_determinate_nix()`
which read the host filesystem. These are not injectable via `MockExecutor` alone.

**Mitigation:** Explicitly scope this backlog item to test only the pathways reachable by the
`runner` parameter. Document that Nix path coverage requires a future `SystemProber` trait
(Backlog Phase D). Note that the nix-profile and channel paths *could* be unit-tested with some
refactoring — these are accepted gaps for now.

### Risk 4: Breaking Changes to `Backend` Trait API

**Risk:** Changing `run_update`'s parameter type from `&CommandRunner` to `&dyn CommandExecutor`
is a semver-breaking change to the `Backend` trait.

**Mitigation:** This project is not a library crate — it's a binary application. All `Backend`
implementors are in the same workspace. The change is mechanical and complete: update all four
backend files. No external consumers exist.

### Risk 5: `Arc<Mutex<VecDeque>>` in `MockExecutor` — Potential Panic on Exhaustion

**Risk:** If a backend's `run_update` calls `runner.run()` more times than the test enqueued
responses, `MockExecutor` panics with "exhausted".

**Mitigation:** This is intentional — it immediately surfaces call count mismatches during test
development. The panic message clearly identifies the cause. Developers must pre-load the exact
number of responses matching `runner.run()` call count in each code path.

### Risk 6: `CommandRunner::run` Name Collision After `impl CommandExecutor`

**Risk:** `CommandRunner` has an inherent `async fn run(...)`. Adding
`impl CommandExecutor for CommandRunner { fn run(...) }` creates two `run` methods with the same
name — one inherent, one from the trait.

**Mitigation:** Rust resolves this via UFCS (Universal Function Call Syntax). Callers using
`runner.run(...)` — both in the impl body and in all backends — will use the inherent method by
default. The trait method is only invoked when dispatched through `dyn CommandExecutor`. The
`impl CommandExecutor` body itself calls `self.run(...)` which resolves to the inherent method.
This is a standard Rust pattern and is safe.

---

## 11. Research Sources

1. **Rust Reference — Dyn compatibility (formerly object safety)**  
   https://doc.rust-lang.org/reference/items/traits.html#object-safety  
   Confirms `async fn` in traits is NOT dyn-compatible; `Pin<Box<dyn Future>>` is the idiomatic
   workaround.

2. **`mockall` crate documentation (v0.14)**  
   https://docs.rs/mockall/latest/mockall/  
   Evaluated for use as mock generator. Supports async traits via `Pin<Box<dyn Future>>` and
   `#[async_trait]`. Rejected for this item in favour of a simpler hand-written double.

3. **The Rust Book — Chapter 10: Traits**  
   https://doc.rust-lang.org/book/ch10-02-traits.html  
   Reference for trait definitions, `impl Trait`, generics, and trait bound syntax.

4. **Rust Async Book — `Pin` and `Future`**  
   https://rust-lang.github.io/async-book/04_pinning/01_chapter.html  
   Explains why `Pin<Box<dyn Future>>` is required for heterogeneous async return types in traits.

5. **`tokio::test` attribute documentation**  
   https://docs.rs/tokio/latest/tokio/attr.test.html  
   Used for `#[tokio::test]` in backend `run_update` tests that `await` futures.

6. **GTK4-rs / libadwaita-rs patterns for business logic separation**  
   https://gtk-rs.org/gtk4-rs/stable/latest/book/  
   Confirms that GTK widgets must not be constructed in tests running outside the main thread.
   The `UpdateResult` type is already a pure-data enum (`Send + Sync`) with no GTK dependencies
   — safe to assert in any test thread.

7. **`thiserror` crate documentation**  
   https://docs.rs/thiserror/latest/thiserror/  
   `BackendError` already uses `thiserror` — no changes needed for error types in this backlog
   item.

8. **Rust `std::sync::Arc` + `Mutex` for interior mutability in `&self` contexts**  
   https://doc.rust-lang.org/std/sync/struct.Mutex.html  
   Used in `MockExecutor` to allow mutable queue access through a shared `&self` reference.

---

## 12. Summary

The `CommandExecutor` backlog item has three tightly related deliverables:

1. **Trait definition** (`src/executor.rs`) — a dyn-compatible `CommandExecutor` trait using
   `Pin<Box<dyn Future>>` return type, consistent with the existing `Backend` trait pattern.

2. **`MockExecutor`** (`src/executor.rs`, `#[cfg(test)]`) — a simple FIFO response-queue double
   using `Arc<Mutex<VecDeque>>`. No `mockall` dependency needed.

3. **`run_update` tests** (in each backend's `#[cfg(test)] mod tests`) — new tests that exercise
   the command → parse → `UpdateResult` pipeline using `MockExecutor`. Parser unit tests already
   exist for all backends; this item adds the execution-path layer on top.

The migration is entirely mechanical: one new file, one `mod` declaration, one `impl` block on
`CommandRunner`, and a single-line signature change in the `Backend` trait and all its
implementors. The `orchestrator.rs` call site requires zero changes.
