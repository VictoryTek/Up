# Specification: Deduplicate Streaming Command Execution

**Feature Name**: `streaming_command_dedup`
**Date**: 2026-03-18
**Status**: Ready for Implementation

---

## 1. Current State Analysis

### 1.1 Function Signatures

**`run_streaming_command()` in `src/upgrade.rs` (private, free function)**

```rust
fn run_streaming_command(program: &str, args: &[&str], tx: &async_channel::Sender<String>) -> bool
```

**`CommandRunner::run()` in `src/runner.rs` (public, async method)**

```rust
pub async fn run(&self, program: &str, args: &[&str]) -> Result<String, String>
```

`CommandRunner` is constructed as:

```rust
pub fn new(tx: async_channel::Sender<(BackendKind, String)>, kind: BackendKind) -> Self
```

### 1.2 Side-by-Side Behaviour Comparison

| Aspect | `run_streaming_command()` | `CommandRunner::run()` |
|---|---|---|
| Execution model | Synchronous (`fn`) | Async (`async fn`) |
| Drain concurrency | Two `std::thread::spawn` drains running concurrently | Two Tokio `async` tasks via `tokio::join!` |
| Stdout draining | `std::io::BufReader` + `lines().map_while(Result::ok)` | `tokio::io::BufReader` + `next_line().await` |
| Stderr draining | Prefixes each line with `"stderr: "` before sending | No prefix; lines sent identically to stdout |
| Channel element type | `async_channel::Sender<String>` | `async_channel::Sender<(BackendKind, String)>` |
| Return type | `bool` (`true` = success) | `Result<String, String>` (`Ok(full_output)` / `Err(message)`) |
| Error communication | Sends error string to channel, returns `false` | Returns `Err(String)` to caller |
| Thread-join safety | `let _ = stdout_thread.join(); let _ = stderr_thread.join()` — **silently discards thread panics** | N/A — Tokio tasks, no `JoinHandle` discard |
| Command echo | None | Sends `$ program args` to channel before spawning |
| Logging | None | Uses `log::info!` and `log::warn!` |
| Success message | Sends `"Command completed successfully."` | No extra message |
| Full output accumulation | No — discards output after forwarding to channel | Yes — accumulates into a `String` returned to caller |

### 1.3 All `run_streaming_command()` Call Sites in `upgrade.rs`

There are **7 call sites** across 4 functions:

#### `fn upgrade_ubuntu(tx: &async_channel::Sender<String>) -> bool`

1. ```rust
   run_streaming_command(
       "pkexec",
       &["do-release-upgrade", "-f", "DistUpgradeViewNonInteractive"],
       tx,
   )
   ```
   — Return value used directly as the function return value.

#### `fn upgrade_fedora(tx: &async_channel::Sender<String>) -> bool`

2. ```rust
   if !run_streaming_command(
       "pkexec",
       &["dnf", "install", "-y", "dnf-plugin-system-upgrade"],
       tx,
   ) { return false; }
   ```
   — Early return on failure.

3. ```rust
   if !run_streaming_command(
       "pkexec",
       &["dnf", "system-upgrade", "download", "--releasever", &ver_str, "-y"],
       tx,
   ) { return false; }
   ```
   — Early return on failure.

4. ```rust
   run_streaming_command("pkexec", &["dnf", "system-upgrade", "reboot"], tx)
   ```
   — Return value used directly.

#### `fn upgrade_opensuse(tx: &async_channel::Sender<String>) -> bool`

5. ```rust
   run_streaming_command("pkexec", &["zypper", "dup", "-y"], tx)
   ```
   — Return value used directly.

#### `fn upgrade_nixos(tx: &async_channel::Sender<String>) -> bool`

LegacyChannel path:

6. ```rust
   // cmd = "export PATH=... && nix-channel --update"
   if !run_streaming_command("pkexec", &["sh", "-c", &cmd], tx) { return false; }
   ```
   — Early return on failure.

7. ```rust
   run_streaming_command("pkexec", &["nixos-rebuild", "switch", "--upgrade"], tx)
   ```
   — Return value used directly.

   Flake path:

8. ```rust
   // cmd = "export PATH=... && nix flake update --flake /etc/nixos"
   if !run_streaming_command("pkexec", &["sh", "-c", &cmd], tx) { return false; }
   ```
   — Early return on failure. (**Note**: This `sh -c` pattern is a separate audit item; it must be preserved as-is in this deduplication change.)

9. ```rust
   run_streaming_command(
       "pkexec",
       &["nixos-rebuild", "switch", "--flake", &flake_target],
       tx,
   )
   ```
   — Return value used directly.

> **Count correction**: `upgrade_nixos` contains 4 call sites split across two `match` arms. Only one arm executes per call, but all 4 are present in the source.  Total call sites in the file: **9**.

### 1.4 Existing `CommandRunner` Usage Pattern in Backend Files

All backend `run_update` implementations receive `runner: &CommandRunner` and invoke:

```rust
runner.run("pkexec", &["some", "args"]).await
```

- `src/backends/os_package_manager.rs`: All four OS backends (`AptBackend`, `DnfBackend`, `PacmanBackend`, `ZypperBackend`) call `runner.run(...).await` inside the `async fn run_update`.
- `src/backends/nix.rs`: `NixBackend::run_update` makes multiple `runner.run(...).await` calls depending on the Nix config type.
- `src/backends/flatpak.rs` and `src/backends/homebrew.rs`: Expected to follow the same pattern.

The `CommandRunner` is constructed once per update cycle by the caller and injected into each backend.

---

## 2. Problem Definition

### 2.1 DRY Violation

`run_streaming_command()` and `CommandRunner::run()` both implement the same fundamental pattern: spawn a child process, concurrently drain stdout and stderr to prevent pipe-buffer deadlock, forward lines to a channel, and report success or failure. Any correctness fix (e.g., improved error reporting, pipe timeout, sanitized output) must currently be applied to **both** implementations independently.

### 2.2 Thread-Join Error Discard (Divergence Bug)

`run_streaming_command()`:

```rust
let _ = stdout_thread.join();
let _ = stderr_thread.join();
```

`std::thread::JoinHandle::join()` returns `Err(payload)` if the thread panicked. Discarding the `Result` with `let _ =` causes panics in the drain threads to be silently swallowed — the process continues as if the command succeeded up to the wait point, potentially misreporting results. `CommandRunner::run()` uses `tokio::join!` with inline async closures and does not have this class of bug.

### 2.3 Other Behavioural Differences to Be Aware Of

- **Stderr prefix**: `run_streaming_command()` prepends `"stderr: "` to all stderr lines. `CommandRunner::run()` does not. This difference is intentional for the upgrade context (stderr from `pkexec`/distro tools should be visually labelled in the UI log panel) and must be preserved.
- **Channel element type**: `upgrade.rs` uses `Sender<String>`; `CommandRunner` uses `Sender<(BackendKind, String)>`. These are incompatible without adaptation.

---

## 3. Proposed Solution Architecture

### 3.1 Approach Assessment

Two approaches were evaluated:

**Approach A — Wrap `CommandRunner::run()` for sync use:**
Add a `run_sync()` method to `CommandRunner` that creates an internal `tokio::runtime::Runtime` and calls `block_on(self.run(...))`.

*Obstacles*:
1. `CommandRunner` stores `Sender<(BackendKind, String)>`. The `upgrade.rs` callers have `Sender<String>`. Bridging requires either a new `BackendKind` variant, a generic parameter, or a wrapper channel — none of which are trivially correct or minimal.
2. `CommandRunner::run()` does not prefix stderr lines with `"stderr: "`. Matching the existing UI behaviour requires special-casing.
3. Creating a new Tokio runtime per command in a worker thread that potentially already nests async work adds complexity for no benefit.

**Approach B (Chosen) — Move `run_streaming_command()` to `runner.rs` as a standalone public function:**
Extract the function verbatim into `src/runner.rs` as `pub fn run_command_sync()`, fix the thread-join discard bug in the same commit, and remove `run_streaming_command()` from `upgrade.rs`.

*Why this is correct*:
1. No channel type adaptation — the function signature stays `(&str, &[&str], &Sender<String>) -> bool`.
2. No Tokio runtime manipulation — the function is synchronous and remains so.
3. Minimal diff — only moves code, deletes one function definition, and updates all call sites to a module-qualified path.
4. Achieves the stated DRY goal: there is now one canonical synchronous streaming command runner, located in `runner.rs`.
5. The thread-join safety fix is naturally included.
6. The `sh -c` call pattern in `upgrade_nixos()` is completely unaffected.

### 3.2 The New Function

Add to `src/runner.rs`:

```rust
/// Run a command synchronously, streaming stdout and stderr line-by-line to
/// `tx`. Stderr lines are prefixed with `"stderr: "`. Returns `true` if the
/// command exits with status 0, `false` on any error.
///
/// Intended for use on `std::thread::spawn` worker threads that do not have a
/// Tokio runtime. Stdout and stderr are drained on separate threads to prevent
/// pipe-buffer deadlock.
pub fn run_command_sync(
    program: &str,
    args: &[&str],
    tx: &async_channel::Sender<String>,
) -> bool {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let result = std::process::Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    match result {
        Ok(mut child) => {
            let stdout_pipe = child.stdout.take();
            let stderr_pipe = child.stderr.take();

            let tx_stdout = tx.clone();
            let stdout_thread = std::thread::spawn(move || {
                if let Some(pipe) = stdout_pipe {
                    let reader = BufReader::new(pipe);
                    for line in reader.lines().map_while(Result::ok) {
                        let _ = tx_stdout.send_blocking(line);
                    }
                }
            });

            let tx_stderr = tx.clone();
            let stderr_thread = std::thread::spawn(move || {
                if let Some(pipe) = stderr_pipe {
                    let reader = BufReader::new(pipe);
                    for line in reader.lines().map_while(Result::ok) {
                        let _ = tx_stderr.send_blocking(format!("stderr: {line}"));
                    }
                }
            });

            // Propagate drain-thread panics rather than silently discarding them.
            if stdout_thread.join().is_err() {
                let _ = tx.send_blocking(
                    "Internal error: stdout drain thread panicked".to_string(),
                );
            }
            if stderr_thread.join().is_err() {
                let _ = tx.send_blocking(
                    "Internal error: stderr drain thread panicked".to_string(),
                );
            }

            match child.wait() {
                Ok(status) => {
                    if status.success() {
                        let _ = tx.send_blocking("Command completed successfully.".into());
                        true
                    } else {
                        let code = status.code().unwrap_or(-1);
                        let _ = tx.send_blocking(format!("Command exited with code {code}"));
                        false
                    }
                }
                Err(e) => {
                    let _ = tx.send_blocking(format!("Failed to wait for process: {e}"));
                    false
                }
            }
        }
        Err(e) => {
            let _ = tx.send_blocking(format!("Failed to start {program}: {e}"));
            false
        }
    }
}
```

### 3.3 Changes to `upgrade.rs`

1. Remove the `run_streaming_command()` function definition entirely (approximately lines 498–560 of the current file).
2. Add a `use crate::runner::run_command_sync;` import at the top of `upgrade.rs` (alongside the existing `use` declarations).
3. Replace every occurrence of `run_streaming_command(` with `run_command_sync(` — no other changes to any call site are required because the signature is identical.

---

## 4. Implementation Steps

1. **Open `src/runner.rs`.**
   - After the closing `}` of the `impl CommandRunner` block, add the `run_command_sync` free function exactly as specified in §3.2.
   - Add the necessary imports at the top of the function body (`std::io::{BufRead, BufReader}`, `std::process::Stdio`) — these should be scoped inside the function body to avoid polluting the module namespace (they are already local in `run_streaming_command()`).

2. **Verify `src/runner.rs` compiles cleanly** with `cargo check` before proceeding.

3. **Open `src/upgrade.rs`.**
   - Add `use crate::runner::run_command_sync;` to the import block at the top of the file.
   - Locate the private `fn run_streaming_command(...)` function definition and **delete it entirely** (the entire function body including the closing `}`).
   - In `upgrade_ubuntu()`: change `run_streaming_command(` → `run_command_sync(`.
   - In `upgrade_fedora()`: change all three occurrences of `run_streaming_command(` → `run_command_sync(`.
   - In `upgrade_opensuse()`: change `run_streaming_command(` → `run_command_sync(`.
   - In `upgrade_nixos()`: change all four occurrences of `run_streaming_command(` → `run_command_sync(`.

4. **Do not modify the `sh -c` call pattern in `upgrade_nixos()`** — that is a separate audit item.

5. **Run `cargo build`** — must succeed with no errors.

6. **Run `cargo clippy -- -D warnings`** — must produce no warnings. Pay attention to any unused-import warnings in `upgrade.rs` if `std::process::Command` (already used elsewhere in the file) or `std::io` were previously only used by the deleted function.

7. **Run `cargo fmt`** — apply formatting to both changed files.

8. **Run `cargo test`** — existing tests must pass without modification.

---

## 5. Dependencies

No new Cargo dependencies are required. The implementation uses only:
- `std::io::{BufRead, BufReader}` (standard library)
- `std::process::{Command, Stdio}` (standard library, already imported in `upgrade.rs`)
- `async_channel` (already a project dependency, already used in `runner.rs` via `CommandRunner`)

Confirmed: **no changes to `Cargo.toml`**.

---

## 6. Affected Files

| File | Change |
|---|---|
| `src/runner.rs` | Add `pub fn run_command_sync()` free function after the `CommandRunner` `impl` block |
| `src/upgrade.rs` | Add `use crate::runner::run_command_sync;`; delete `fn run_streaming_command()`; update 9 call sites |

No other files require changes. `src/ui/upgrade_page.rs` is not affected — it calls `upgrade::execute_upgrade()` whose signature does not change.

---

## 7. Risks and Mitigations

### Risk 1: `CommandRunner` requires `Sender<(BackendKind, String)>` — not `Sender<String>`

**Context**: The spec prompt raised the question of whether `execute_upgrade()` callees could receive a `CommandRunner` and call `.run().await`. They cannot as-is because of this type mismatch.

**Resolution**: The chosen approach (Approach B) does **not** use `CommandRunner` in `upgrade.rs`. The new `run_command_sync()` function is a standalone free function that accepts `&Sender<String>`, exactly matching the existing upgrade callpath. No `CommandRunner` is constructed or passed.

### Risk 2: Return type mismatch (`Result<String, String>` vs `bool`)

**Context**: If a future refactor wished to use `CommandRunner::run()` in `upgrade.rs`, the `Ok/Err` result would need translation to `bool`.

**Resolution**: Not applicable to the chosen approach. `run_command_sync()` returns `bool` directly. No translation is needed.

### Risk 3: Silent thread-panic discard bug introduced in `run_command_sync()`

**Context**: The existing `run_streaming_command()` silently discards drain-thread panics with `let _ = join()`. Copying it verbatim would perpetuate this bug.

**Mitigation**: The new `run_command_sync()` explicitly checks the `join()` result and sends an error message to the channel if a drain thread panicked. This is a deliberate improvement included in the move.

### Risk 4: Behavioural regression — stderr prefix

**Context**: `run_streaming_command()` prefixes stderr lines with `"stderr: "`. If this prefix were accidentally lost, stderr lines from distro upgrade tools would be indistinguishable from stdout in the UI log panel.

**Mitigation**: The implementation in §3.2 preserves the `format!("stderr: {line}")` prefix exactly as it appears in the current function.

### Risk 5: The `sh -c` pattern in `upgrade_nixos()` is not changed

**Context**: The audit noted that `upgrade_nixos()` uses `run_streaming_command("pkexec", &["sh", "-c", &cmd], tx)` — a `sh -c` invocation that embeds a PATH export. This is a distinct security audit item (shell injection surface).

**Mitigation**: This spec and its resulting implementation make **no change** to how `upgrade_nixos()` constructs commands. The `sh -c` calls are renamed from `run_streaming_command` to `run_command_sync` and left otherwise unmodified. The shell-injection audit item must be addressed in a separate, dedicated spec.

### Risk 6: Moving the function introduces a compile error if `std::process::Command` import ambiguity exists

**Context**: `upgrade.rs` imports `use std::process::Command` at the top. After deleting `run_streaming_command()`, if `std::process::Command` is no longer used elsewhere in `upgrade.rs`, `cargo clippy` will warn about the unused import.

**Mitigation**: Check whether other code in `upgrade.rs` still uses `std::process::Command` (it does — `check_packages_up_to_date`, `check_disk_space`, `check_ubuntu_upgrade`, etc. all use it). The import will remain valid and should not need removal.

---

## Summary

`run_streaming_command()` in `upgrade.rs` and `CommandRunner::run()` in `runner.rs` implement the same spawn-drain-report loop with minor but meaningful differences (sync vs async, channel element type, stderr labelling). Full unification through `CommandRunner` is blocked by a channel type mismatch and the absence of a Tokio runtime on the worker thread that calls `execute_upgrade`. The minimal, correct deduplication is to relocate `run_streaming_command()` into `runner.rs` as a public free function named `run_command_sync`, fix the silent thread-panic discard in the same commit, and update all 9 call sites in `upgrade.rs`. No new dependencies, no API changes visible outside `upgrade.rs`.
