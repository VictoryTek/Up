# Security Logging Improvement — Phase 1 Specification

**Finding**: OWASP A09 — Insufficient Security-Relevant Event Logging  
**Severity**: LOW  
**Scope**: Minimal hygiene fix — replace `eprintln!` error output with `log::error!`/`log::warn!`, add `log::info!` at security-relevant event sites, and upgrade `env_logger` initialisation to a sensible default filter level.

---

## 1. Current State Analysis

### 1.1 `env_logger` Initialisation

`env_logger` is already called in `src/main.rs` (line 13):

```rust
env_logger::init();
```

This bare form silences all output unless the user explicitly sets `RUST_LOG`. It should be replaced with the builder form that defaults to `warn` when `RUST_LOG` is unset. Both `log = "0.4"` and `env_logger = "0.11"` are already declared in `Cargo.toml` — no new dependencies are needed.

### 1.2 `eprintln!` / `println!` Audit

A full scan of `src/**/*.rs` found exactly **two** `eprintln!` calls, both in `src/reboot.rs`:

| File | Line | Content |
|------|------|---------|
| `src/reboot.rs` | 14 | `eprintln!("Failed to spawn reboot command: {e}");` (Flatpak path) |
| `src/reboot.rs` | 17 | `eprintln!("Failed to spawn reboot command: {e}");` (direct path) |

No `println!` calls exist anywhere in the source tree. No other `eprintln!` calls exist.

### 1.3 `pkexec` Invocation Sites

All `pkexec` invocations flow through `runner.rs`'s `CommandRunner::run()` method — there is no direct `Command::new("pkexec")` outside the runner. The call sites are:

| Backend File | Method | Commands via `runner.run("pkexec", ...)` |
|---|---|---|
| `os_package_manager.rs` | `AptBackend::run_update` | `apt update`, `apt upgrade -y` |
| `os_package_manager.rs` | `DnfBackend::run_update` | `dnf upgrade -y` |
| `os_package_manager.rs` | `PacmanBackend::run_update` | `pacman -Syu --noconfirm` |
| `os_package_manager.rs` | `ZypperBackend::run_update` | `zypper refresh`, `zypper update -y` |
| `nix.rs` | `NixBackend::run_update` | `nix flake update` (flake path), `nixos-rebuild switch --flake` (flake path), `nixos-rebuild switch --upgrade` (legacy path) |

Since every `pkexec` invocation passes through `CommandRunner::run()`, adding a single `log::info!` there covers all privilege-escalation events without any per-backend changes.

### 1.4 Current `log` Crate Usage

Neither `use log::...` nor any `log::` macro is called anywhere in `src/`. The crate is linked but dormant.

---

## 2. Problem Definition

- Security-relevant events (pkexec invocations, command failures, reboot requests, backend detection) emit no structured log output.
- `eprintln!` in `reboot.rs` writes to stderr which is discarded in most GUI deployments.
- `env_logger::init()` makes the logger silent by default, so even if macros were added they would produce no output at the standard log level.
- OWASP A09 requires that failure events and privilege-escalation events be observable via logs.

---

## 3. Proposed Solution Architecture

Minimal, four-file change:

1. **`src/main.rs`** — upgrade `env_logger::init()` to use `Builder::from_env` with `default_filter_or("warn")`.
2. **`src/runner.rs`** — add `log::info!` before every command is spawned (covers all `pkexec` calls centrally) and `log::warn!` when a command exits non-zero.
3. **`src/backends/mod.rs`** — add `log::info!` for each backend detected by `detect_backends()`.
4. **`src/reboot.rs`** — add `log::info!("Reboot requested")` before the reboot is issued, and replace both `eprintln!` calls with `log::error!`.

No logic changes, no new dependencies, no new modules, no `#[instrument]`.

---

## 4. Complete Change Table

| # | File | Location | Current code | Replacement | Level |
|---|------|----------|--------------|-------------|-------|
| 1 | `src/main.rs` | Line 13 — `env_logger::init()` | `env_logger::init();` | `env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();` | n/a (init) |
| 2 | `src/runner.rs` | Top of file — imports | _(no log import)_ | `use log::{info, warn};` | n/a (import) |
| 3 | `src/runner.rs` | Inside `run()`, after `self.send(format!("$ {display_cmd}")).await;` and before `let mut child = Command::new(...)` | _(nothing)_ | `info!("Running: {} {:?}", program, args);` | INFO |
| 4 | `src/runner.rs` | Inside `run()`, `else` branch of `if status.success()` — before the `Err(...)` | _(nothing, just `Err(...)`)_ | `warn!("{program} exited with code {code}");` (then existing `Err(...)`) | WARN |
| 5 | `src/backends/mod.rs` | Top of file — imports | _(no log import)_ | `use log::info;` | n/a (import) |
| 6 | `src/backends/mod.rs` | Inside `detect_backends()`, immediately before `backends` (the final `backends` return statement) | _(nothing before return)_ | `for b in &backends { info!("Backend detected: {}", b.display_name()); }` | INFO |
| 7 | `src/reboot.rs` | Top of file — imports | _(no log import)_ | `use log::{error, info};` | n/a (import) |
| 8 | `src/reboot.rs` | Inside `reboot()`, before the `if Path::new("/.flatpak-info").exists()` branch | _(nothing)_ | `info!("Reboot requested");` | INFO |
| 9 | `src/reboot.rs` | Line 14 — Flatpak error path | `eprintln!("Failed to spawn reboot command: {e}");` | `error!("Failed to spawn reboot command: {e}");` | ERROR |
| 10 | `src/reboot.rs` | Line 17 — direct path | `eprintln!("Failed to spawn reboot command: {e}");` | `error!("Failed to spawn reboot command: {e}");` | ERROR |

---

## 5. Exact Implementation Details

### 5.1 `src/main.rs` — `env_logger` initialisation

**Current** (line 13):
```rust
env_logger::init();
```

**Replacement**:
```rust
env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
```

No `use` import is required — `env_logger` is already used by its full path.

**Rationale**: `default_filter_or("warn")` means the logger emits `WARN` and `ERROR` messages when `RUST_LOG` is unset (the common case in a desktop deployment), while still allowing the user to raise verbosity with `RUST_LOG=up=info` or `RUST_LOG=debug` when needed.

---

### 5.2 `src/runner.rs` — command invocation logging

Add `use log::{info, warn};` to the imports block.

Inside `CommandRunner::run()`, insert after the `self.send(...)` call and before `let mut child = ...`:

```rust
info!("Running: {} {:?}", program, args);
```

Inside the `else` branch of `if status.success()`, insert before the `Err(...)` return:

```rust
let code = status.code().unwrap_or(-1);
warn!("{program} exited with code {code}");
Err(format!("{program} exited with code {code}"))
```

*(The `let code` line is already present — only the `warn!` line is new.)*

**Rationale**: Every call to `pkexec` (and every other command) passes through this method. A single `info!` here therefore covers all privilege-escalation invocations. The `warn!` on non-zero exit satisfies the requirement to log unexpected exit codes.

---

### 5.3 `src/backends/mod.rs` — backend detection

Add `use log::info;` to the imports block.

Inside `detect_backends()`, immediately before the final `backends` expression (the implicit return), insert:

```rust
for b in &backends {
    info!("Backend detected: {}", b.display_name());
}
```

**Rationale**: Logs which package managers were found at startup. Between zero and four backends can be detected; the loop is O(n) on a tiny slice and has no performance impact.

---

### 5.4 `src/reboot.rs` — reboot request logging

Add `use log::{error, info};` to the imports block.

Inside `reboot()`, as the first statement (before the `if` branching on Flatpak):

```rust
info!("Reboot requested");
```

Replace both `eprintln!("Failed to spawn reboot command: {e}");` lines with:

```rust
error!("Failed to spawn reboot command: {e}");
```

**Rationale**: A reboot request is a security-relevant event that should be observable. Spawn failures are errors, not informational output.

---

## 6. `use log::{...}` Imports Needed Per File

| File | Import to add |
|------|--------------|
| `src/runner.rs` | `use log::{info, warn};` |
| `src/backends/mod.rs` | `use log::info;` |
| `src/reboot.rs` | `use log::{error, info};` |

`src/main.rs` requires no new `use` statement — `env_logger` is called by full path.

---

## 7. What NOT to Change

The following `eprintln!`/`println!`-equivalent calls are **UI channels or informational strings**, not error reporting, and must be left untouched:

| Location | Code | Reason |
|----------|------|--------|
| `src/runner.rs` line ~21 | `self.send(format!("$ {display_cmd}")).await;` | Sends command echo to the in-app log panel — UI output, not system log |
| `src/runner.rs` `stdout_task` / `stderr_task` | `tx_stdout.send(...)` / `tx_stderr.send(...)` | Streaming child output to UI — not log calls |
| `src/upgrade.rs` all functions | `format!(...)` / `String::from(...)` return values | Business-logic strings returned for UI display — not logging |
| `src/upgrade.rs` `check_*` helpers | `tx.send_blocking("Checking...")` | Progress messages to the upgrade UI channel — not system log |
| `src/ui/window.rs` | All GTK widget setup | Pure UI code — no logging needed |
| `src/ui/upgrade_page.rs` | All GTK widget setup | Pure UI code — no logging needed |
| `src/backends/os_package_manager.rs` | `tx.send_blocking("Checking...")` | Progress messages to upgrade UI channel — not system log |

---

## 8. Implementation Steps (for Phase 2)

1. Open `src/main.rs`; replace `env_logger::init()` with the Builder form.
2. Open `src/runner.rs`; add `use log::{info, warn};`; add `info!` before spawn; add `warn!` in the non-zero exit branch.
3. Open `src/backends/mod.rs`; add `use log::info;`; add detection loop before `backends` return.
4. Open `src/reboot.rs`; add `use log::{error, info};`; add `info!("Reboot requested")` at the top of `reboot()`; replace both `eprintln!` with `error!`.
5. Run `cargo build` — must compile cleanly.
6. Run `cargo clippy -- -D warnings` — must produce no warnings.
7. Run `cargo fmt --check` — must pass.

---

## 9. Dependencies

No new dependencies are introduced. Both `log = "0.4"` and `env_logger = "0.11"` are already present in `Cargo.toml`.

---

## 10. Risks and Mitigations

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| `info!` in runner leaks sensitive args to system log | Low — no args are sensitive in this project; all are package-manager subcommands | Acceptable. If future backends add credential args, they must mask them before passing to runner. |
| `default_filter_or("warn")` hides INFO logs in production without `RUST_LOG` | Intended | Users who want verbose output set `RUST_LOG=up=info`. |
| `warn!` in runner duplicates the `Err(...)` return message | Low — they are separate channels (log vs Result) | Acceptable; the duplication is harmless and explicit separation of log output from error propagation is good practice. |

---

## 11. Verification

After implementation, verify logging works:

```bash
# Show WARN and above (default in production deployments)
cargo run

# Show INFO events — backend detection, pkexec invocations, reboot requests
RUST_LOG=up=info cargo run

# Show all DEBUG events
RUST_LOG=up=debug cargo run

# Show logs only from env_logger's own crate (useful for diagnosing init issues)
RUST_LOG=env_logger=debug cargo run
```

Expected output at `RUST_LOG=up=info` after clicking "Update All" on a system with APT:

```
[INFO  up::backends] Backend detected: APT
[INFO  up::runner] Running: "pkexec" ["env", "DEBIAN_FRONTEND=noninteractive", "apt", "update"]
[INFO  up::runner] Running: "pkexec" ["env", "DEBIAN_FRONTEND=noninteractive", "apt", "upgrade", "-y"]
```

Expected output from `reboot.rs` when a reboot is triggered:

```
[INFO  up::reboot] Reboot requested
```

Expected output when a command fails (e.g., `pkexec` is cancelled by the user):

```
[WARN  up::runner] pkexec exited with code 126
```
