# Performance Fixes — Specification

**Feature name:** performance  
**Date:** 2026-05-07  
**Scope:** `src/runtime.rs`, `src/ui/mod.rs`, `src/orchestrator.rs`, `src/runner.rs`, `Cargo.toml`

---

## Summary of Findings

Three performance items were requested. Detailed code inspection reveals the
following:

| Item | Status | Action |
|------|--------|--------|
| 6.1 Add `rt-multi-thread` to Cargo.toml | **Already done** | No change needed |
| 6.2 Shared process-wide runtime in `ui/mod.rs` | **Already done** | No change needed |
| 5. Orchestrator uses shared runtime | **Already done** | No change needed |
| 6.6 Avoid full-output accumulation in `runner.rs` | **Not done** | Implementation required |

---

## Item 1 — Shared Tokio Runtime (6.1 / 6.2 / 5)

### Current State

`src/runtime.rs` already exists and already provides a process-wide, multi-thread
runtime via `OnceLock`:

```rust
// src/runtime.rs (current)
use std::sync::OnceLock;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

pub fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build Tokio runtime")
    })
}
```

`Cargo.toml` already includes `rt-multi-thread` alongside the other runtime
features:

```toml
tokio = { version = "1", features = ["rt", "rt-multi-thread", "macros",
          "io-util", "process", "fs", "sync", "time"] }
```

`src/ui/mod.rs` already routes every async background task through this
runtime:

```rust
pub(crate) fn spawn_background_async<F, Fut>(f: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    drop(crate::runtime::runtime().spawn(f()));
}
```

`src/orchestrator.rs` uses an identical private wrapper:

```rust
fn spawn_background<F, Fut>(f: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    drop(crate::runtime::runtime().spawn(f()));
}
```

The GTK main loop runs on the main thread; the Tokio worker threads are
separate, so the GTK event loop is never blocked.

### Verdict

All three sub-items are fully implemented. **No implementation work required.**

---

## Item 2 — Avoid Full Output Accumulation in `runner.rs` (6.6)

### Current State

`CommandRunner::run()` in `src/runner.rs` accumulates every line of stdout and
every line of stderr into an owned `String` while concurrently forwarding them
to the UI channel:

```rust
// stdout_task — current
let stdout_task = async move {
    let mut out = String::new();
    if let Some(pipe) = stdout {
        let mut reader = BufReader::new(pipe).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            out.push_str(&line);
            out.push('\n');
            let _ = tx_stdout.send(BackendEvent::LogLine(kind_stdout, line)).await;
        }
    }
    out
};
```

After `tokio::join!(stdout_task, stderr_task)` both strings are concatenated:

```rust
let (stdout_output, stderr_output) = tokio::join!(stdout_task, stderr_task);
let full_output = stdout_output + &stderr_output;
```

On command success `Ok(full_output)` is returned. Backends consume this to
count upgraded packages:

```rust
// AptBackend
Ok(output) => {
    let count = count_apt_upgraded(&output);   // scans for "N upgraded,"
    UpdateResult::Success { updated_count: count }
}
// DnfBackend — count_dnf_upgraded(&output)
// PacmanBackend — count_pacman_upgraded(&output)
// ZypperBackend — equivalent helper
```

On command failure `full_output` is **discarded**; the error contains only the
exit code:

```rust
Err(BackendError::Exit {
    code,
    message: format!("{program} exited with code {code}"),
})
```

`PrivilegedShell::run_command()` has the same pattern: a `full_output: String`
is accumulated inside the timeout loop and returned on success.

### Problem

For a `dnf upgrade -y` run that takes 30 minutes and produces 50 000+ lines of
RPM transaction output, both the stdout and the stderr buffers become multi-MB
heap allocations that are held until the command finishes. The data has already
been forwarded to the UI via the channel; the in-memory copy serves only as
input to the counting helper that inspects the final summary lines.

### Key Insight

All four counting helpers (`count_apt_upgraded`, `count_dnf_upgraded`,
`count_pacman_upgraded`, and equivalents) look for a **summary line that
appears near the very end of the command's output**. Examples:

- APT: `"2 upgraded, 0 newly installed, ..."`  — last line of `apt upgrade`
- DNF4: `"  Upgrade  15 Packages"` — transaction summary near the end
- DNF5: `"  Upgrading: 15 packages"` — same position
- Pacman: `"(15/15) upgrading ..."`-style lines at the tail

Keeping the **last 100 lines** of stdout and the **last 100 lines** of stderr
in a ring buffer is sufficient to capture these summary lines while bounding
heap usage to roughly 100 × ~120 bytes ≈ 12 KB per stream regardless of how
long the command runs.

### Proposed Fix

#### 2a. `CommandRunner::run()` in `src/runner.rs`

Add a module-level constant:

```rust
/// Maximum number of tail lines retained in memory per stream for error
/// context and post-process parsing.  All lines are still forwarded to the
/// UI in real time through the async channel.
const OUTPUT_TAIL_LINES: usize = 100;
```

Replace the `stdout_task` and `stderr_task` closures. The accumulation type
changes from `String` to `VecDeque<String>`; the ring-buffer draining joins the
lines back into a `String` at the end (the returned value from the task):

```rust
use std::collections::VecDeque;

let stdout_task = async move {
    let mut tail: VecDeque<String> = VecDeque::with_capacity(OUTPUT_TAIL_LINES + 1);
    if let Some(pipe) = stdout {
        let mut reader = BufReader::new(pipe).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if tail.len() == OUTPUT_TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line.clone());
            let _ = tx_stdout
                .send(BackendEvent::LogLine(kind_stdout, line))
                .await;
        }
    }
    tail.into_iter().collect::<Vec<_>>().join("\n")
};

let stderr_task = async move {
    let mut tail: VecDeque<String> = VecDeque::with_capacity(OUTPUT_TAIL_LINES + 1);
    if let Some(pipe) = stderr {
        let mut reader = BufReader::new(pipe).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if tail.len() == OUTPUT_TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line.clone());
            let _ = tx_stderr
                .send(BackendEvent::LogLine(kind_stderr, line))
                .await;
        }
    }
    tail.into_iter().collect::<Vec<_>>().join("\n")
};
```

The rest of the function (`tokio::join!`, `full_output`, exit status check,
return value) remains unchanged. The only change is how much is buffered in
memory; the returned `String` is at most `OUTPUT_TAIL_LINES × ~120` bytes on
both the success and error paths.

#### 2b. `PrivilegedShell::run_command()` in `src/runner.rs`

The same pattern applies to the privileged shell's output accumulator. Replace
the current `full_output: String` with a bounded `VecDeque<String>`:

Current code (inside the `tokio::time::timeout` block):

```rust
let mut full_output = String::new();
loop {
    let mut line = String::new();
    let n = self.reader.read_line(&mut line).await
        .map_err(|e| format!("Failed to read output: {e}"))?;
    if n == 0 {
        return Err("Privileged shell closed unexpectedly".to_string());
    }
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix(&rc_prefix) {
        if let Some(code_str) = rest.strip_suffix(rc_suffix) {
            let code: i32 = code_str.parse().unwrap_or(-1);
            if code == 0 {
                return Ok(full_output);
            }
            return Err(format!("Command exited with code {code}"));
        }
    }
    let content = line.trim_end_matches('\n').to_string();
    full_output.push_str(&content);
    full_output.push('\n');
    let _ = tx.send(BackendEvent::LogLine(kind, content)).await;
}
```

Proposed replacement:

```rust
let mut tail: VecDeque<String> = VecDeque::with_capacity(OUTPUT_TAIL_LINES + 1);
loop {
    let mut line = String::new();
    let n = self.reader.read_line(&mut line).await
        .map_err(|e| format!("Failed to read output: {e}"))?;
    if n == 0 {
        return Err("Privileged shell closed unexpectedly".to_string());
    }
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix(&rc_prefix) {
        if let Some(code_str) = rest.strip_suffix(rc_suffix) {
            let code: i32 = code_str.parse().unwrap_or(-1);
            let tail_str = tail.into_iter().collect::<Vec<_>>().join("\n");
            if code == 0 {
                return Ok(tail_str);
            }
            return Err(format!("Command exited with code {code}"));
        }
    }
    let content = line.trim_end_matches('\n').to_string();
    if tail.len() == OUTPUT_TAIL_LINES {
        tail.pop_front();
    }
    tail.push_back(content.clone());
    let _ = tx.send(BackendEvent::LogLine(kind, content)).await;
}
```

Note: `OUTPUT_TAIL_LINES` is defined at the top of the module (see 2a above) and
is accessible to both functions in `runner.rs`.

#### 2c. Required import addition

`VecDeque` is in the standard library; add to the existing `use` block at the
top of `src/runner.rs`:

```rust
use std::collections::VecDeque;
```

### Files Affected

| File | Change |
|------|--------|
| `src/runner.rs` | Add `VecDeque` import; add `OUTPUT_TAIL_LINES` constant; replace `String` accumulation with `VecDeque` ring buffer in `CommandRunner::run()` and `PrivilegedShell::run_command()` |

No other files require changes. The public API surface (`CommandExecutor` trait,
`BackendError`, `UpdateResult`, `OrchestratorEvent`) is unchanged.

### Memory Impact

| Scenario | Before | After |
|----------|--------|-------|
| 30-min DNF upgrade (~50 000 lines) | ~6 MB stdout + ~1 MB stderr held until command exits | ≤ 24 KB per stream (100 lines × ~120 bytes) |
| Normal APT upgrade (~200 lines) | ~24 KB | ~24 KB (no visible difference at this scale) |
| Failed command | Same as success (buffer built, then discarded) | Same as success path — ring buffer is the same size |

### Risks and Mitigations

| Risk | Likelihood | Mitigation |
|------|------------|------------|
| Counting helpers miss the summary line because it falls outside the 100-line tail | Very low — summary lines appear at the very end of output; 100 lines is orders of magnitude more than needed (APT summary is the last line, DNF summary is ≤ 10 lines from the end) | If ever an edge case arises, `OUTPUT_TAIL_LINES` is a single constant; increase to 200 |
| `VecDeque::with_capacity` pre-allocates 101 entries — minor overhead for short commands | Negligible | Accept; heap fragmentation is insignificant for 100-entry VecDeques |
| `PrivilegedShell::run_command` change: the error path no longer includes full_output in the returned Err | The current code also does not include full output in the Err (it returns only the exit code); behaviour is identical | No additional mitigation needed |

---

## Implementation Steps

1. Open `src/runner.rs`.
2. Add `use std::collections::VecDeque;` to the existing `use` block.
3. Add the `OUTPUT_TAIL_LINES` constant immediately after the imports.
4. In `CommandRunner::run()`: replace the `stdout_task` and `stderr_task`
   closure bodies with the ring-buffer versions shown in §2a. The join,
   concatenation, wait, and return statements are untouched.
5. In `PrivilegedShell::run_command()`: replace the `full_output: String` local
   with a `VecDeque<String>` ring-buffer as shown in §2b.
6. Run `cargo fmt` to normalise formatting.
7. Run `cargo clippy -- -D warnings` to verify no new lint issues.
8. Run `cargo build` to confirm successful compilation.
9. Run `cargo test` to confirm all tests pass.

---

## Non-Goals

- Changing the `CommandExecutor` trait signature.
- Changing how backends parse output (the tail approach is backward-compatible).
- Changing `run_command_sync` in `runner.rs` (that function forwards lines via
  channel with no accumulation; it is already efficient).
- Any changes related to items 6.1, 6.2, or 5 (already implemented).
