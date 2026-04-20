# Update Sequencing Fix — Specification

**Feature:** `update_sequencing_fix`  
**Date:** 2026-04-20  
**Status:** Ready for Implementation

---

## 1. Current State Analysis

### 1.1 Backend Execution Path

Backends are triggered from the **Update All** button click handler in
`src/ui/window.rs` (line 234). The click handler:

1. Wraps all work in `glib::spawn_future_local` so it runs on the GTK main thread
   (line 255).
2. Sorts backends: `needs_root = true` first (line 264).
3. Creates **three separate** async channels (lines 266–269):
   - `(tx, rx)` — `async_channel::unbounded::<(BackendKind, String)>()` — log lines
   - `(result_tx, result_rx)` — `async_channel::unbounded::<(BackendKind, UpdateResult)>()` — results
   - `(started_tx, started_rx)` — `async_channel::unbounded::<BackendKind>()` — "backend started" signals
4. Spawns a worker via `super::spawn_background_async` (line 285), which creates
   its own OS thread + single-threaded Tokio runtime.
5. Inside the worker (lines 303–309), backends execute **sequentially**:

   ```rust
   for backend in &ordered_backends {
       let kind = backend.kind();
       let _ = started_tx_thread.send(kind).await;           // (A)
       let runner = CommandRunner::new(tx_thread.clone(), kind, shell.clone());
       let result = backend.run_update(&runner).await;       // (B)
       let _ = result_tx_thread.send((kind, result)).await;  // (C)
   }
   ```

6. After the worker is spawned, the GTK main coroutine:
   - Awaits `auth_status_rx` (line 321) — yields to the GLib event loop while waiting.
   - Spawns a **separate** `glib::spawn_future_local` future to drain `started_rx`
     (line 347).
   - Spawns another **separate** `glib::spawn_future_local` future to drain `rx`
     (line 358).
   - Enters its own `while let Ok((kind, result)) = result_rx.recv().await` loop
     starting at approximately line 365.

### 1.2 CommandRunner / PrivilegedShell

`src/runner.rs` — `CommandRunner::run` (line ~177):

- If `program == "pkexec"` **and** a `PrivilegedShell` is available, the call
  is routed through the already-elevated shell (line ~191).
- `PrivilegedShell::run_command` (line ~86) writes the quoted command to the
  `pkexec /bin/sh` stdin, then reads lines one-at-a-time via
  `BufReader::read_line`, sending each line through `tx` as
  `(BackendKind, String)`.
- For all other programs, `CommandRunner::run` (line ~202) spawns the process
  directly with `Stdio::piped()` for both stdout and stderr, draining both
  pipes concurrently with `tokio::join!`.

### 1.3 Nix Backend

`src/backends/nix.rs` — `NixBackend::run_update`:

- **NixOS flake** (line ~294): calls
  `runner.run("pkexec", &["env", "PATH=...", "sh", "-c", &cmd])` where `cmd`
  combines `nix flake update --flake /etc/nixos && nixos-rebuild switch
  --flake /etc/nixos#<attr>`.
- **NixOS legacy channels** (line ~318): calls `runner.run("pkexec", &["env",
  "PATH=...", "sh", "-c", "nix-channel --update && nixos-rebuild switch"])`.
- **Non-NixOS profile (flake)**: calls `runner.run("nix", &["profile",
  "upgrade", ".*"])`.
- **Non-NixOS profile (legacy)**: calls `runner.run("nix-env", &["-u"])`.

The `PrivilegedShell` is spawned as `pkexec /bin/sh` with:
```
.stdin(Stdio::piped())
.stdout(Stdio::piped())
.stderr(Stdio::inherit())   ← stderr is NOT captured
```
(`src/runner.rs`, line ~33)

---

## 2. Bug 1 — Race Condition: Parallel Appearance of Nix and Flatpak

### 2.1 Root Cause

The worker runs backends **strictly sequentially**. However, on the GTK main
thread, the three channels are consumed by **three independent futures**:

| Future | Channel consumed | Spawned via |
|--------|-----------------|-------------|
| `started_rx` drain | `started_rx` | `glib::spawn_future_local` (line 347) |
| `rx` drain | `rx` (log lines) | `glib::spawn_future_local` (line 358) |
| `result_rx` loop | `result_rx` | main coroutine, lines ~365+ |

The GLib cooperative scheduler does not guarantee the order in which it
polls these three futures. This creates the following race:

```
Worker timeline (sequential):
  t=0  → send started(Nix)      → started_rx queue: [Nix]
  t=1  → run Nix (fast or slow)
  t=T  → send result(Nix,OK)    → result_rx queue: [Nix/OK]
  t=T  → send started(Flatpak)  → started_rx queue: [Nix, Flatpak]
  t=T+ → run Flatpak

GTK main thread (after auth completes, futures just spawned):
  Poll started_rx → receives started(Nix)    → set Nix to "Running"
  Poll started_rx → receives started(Flatpak)→ set Flatpak to "Running"  ← BUG
  Poll result_rx  → receives result(Nix,OK)  → set Nix to "Success"
```

Because `started_rx` has TWO items already queued by the time the GTK thread
first polls it (this can happen whenever Nix completes before the auth-wait
yields, or during the brief window between auth completion and future
scheduling), the `started_rx` future eagerly processes **both** `started(Nix)`
and `started(Flatpak)` in a single scheduling run. The result loop hasn't had
a chance to process `result(Nix, OK)` yet. For a brief but visible interval,
both rows display as "Updating…" simultaneously.

**Exact offending locations:**
- `src/ui/window.rs:269` — `started_tx` / `started_rx` declared as a separate
  channel instead of being unified with `result_tx`.
- `src/ui/window.rs:347` — `started_rx` processed by an independent future with
  no ordering relationship to the `result_rx` loop.

### 2.2 Proposed Fix

Introduce a single **`BackendEvent`** enum that unifies all three event streams:

```rust
enum BackendEvent {
    Started(BackendKind),
    LogLine(BackendKind, String),
    Finished(BackendKind, UpdateResult),
}
```

Use a single `async_channel::unbounded::<BackendEvent>()` channel. The worker
sends events through this single channel in the exact order they occur:

```rust
// Worker sends (in order, on the same channel):
event_tx.send(BackendEvent::Started(kind)).await;
// ... (log lines arrive via runner, also sent as BackendEvent::LogLine)
event_tx.send(BackendEvent::Finished(kind, result)).await;
```

The GTK main thread has a **single** event loop that processes events in
guaranteed arrival order:

```rust
while let Ok(event) = event_rx.recv().await {
    match event {
        BackendEvent::Started(kind)         => { /* set_status_running */ }
        BackendEvent::LogLine(kind, line)   => { /* append_line */ }
        BackendEvent::Finished(kind, result) => { /* set_status_success/error */ }
    }
}
```

Since `Finished(Nix)` is sent **before** `Started(Flatpak)` on the same
channel, it is guaranteed to be processed first. The race is eliminated
entirely.

**Impact on `CommandRunner`:** The runner's `tx` type must change from
`async_channel::Sender<(BackendKind, String)>` to
`async_channel::Sender<BackendEvent>`, and each send in `PrivilegedShell` and
`CommandRunner` must wrap the payload in `BackendEvent::LogLine(kind, line)`.

---

## 3. Bug 2 — Missing Nix Terminal Output

### 3.1 Root Cause

**3.1.1 Full stdio buffering (primary cause)**

`PrivilegedShell` spawns `pkexec /bin/sh` with `stdout: Stdio::piped()`.
When a process's stdout is connected to a pipe (not a TTY), the C standard
library uses **full buffering** (block buffering, default ~8 KB) instead of
line buffering. The `nix`, `nixos-rebuild`, and `nix-channel` binaries are
C/C++ programs that inherit this behavior.

Consequence: `PrivilegedShell::run_command` calls `BufReader::read_line`
which blocks waiting for data on the pipe. Because the subprocess buffers all
output internally until either the buffer fills (~8 KB) or the command exits,
`read_line` returns nothing until one of those conditions is met. For
`nixos-rebuild switch`, which may run for 10–30 minutes, this means the log
panel displays **zero output** for the entire duration, with all lines
appearing in a single burst at the very end.

**3.1.2 TTY detection — suppressed progress output (secondary cause)**

`nix` (both the legacy and new CLI) calls `isatty(2)` (stderr file descriptor)
at startup. When stderr is not a TTY, nix suppresses:
- The progress bar / spinner
- The `these N derivations will be built` header (normally written to stderr)
- ANSI colour codes

For `nix profile upgrade` and `nix-env -u` (non-NixOS path, direct command
via `CommandRunner::run`), both stdout and stderr are `Stdio::piped()`, so
`isatty` returns false. Nix still produces output (package names, completion
messages) but it is sparse compared to a terminal session. Users who expect
verbose build progress see almost nothing.

**3.1.3 `nixos-rebuild` build logs not enabled (tertiary cause)**

`nixos-rebuild switch` by default suppresses build logs unless
`--print-build-logs` / `-L` is passed. Without this flag, the full nix build
output (fetched store paths, compilation, etc.) never appears at all, even
when buffering is resolved.

**Exact offending locations:**
- `src/runner.rs:33` — `PrivilegedShell::new()` sets `.stderr(Stdio::inherit())`
  and `.stdout(Stdio::piped())` — the shell lives in pipe mode, propagating
  non-TTY context to all child processes.
- `src/backends/nix.rs:296` — flake NixOS command does not include
  `--print-build-logs`.
- `src/backends/nix.rs:321` — legacy channel NixOS command does not include
  `--print-build-logs`.

### 3.2 Proposed Fix

**3.2.1 Force line buffering with `stdbuf`**

Prefix each nix/nixos-rebuild invocation with `stdbuf -oL -eL`. `stdbuf` is
part of GNU coreutils and is always present on NixOS (`/run/current-system/sw/bin/stdbuf`).
It forces the child process's libc stdout and stderr to use line buffering,
causing each line to be flushed to the pipe immediately after it is written.

Modified flake command (assembled inside `src/backends/nix.rs`):
```rust
let cmd = format!(
    "stdbuf -oL -eL \
     nix --extra-experimental-features 'nix-command flakes' \
     flake update --flake /etc/nixos && \
     stdbuf -oL -eL \
     nixos-rebuild switch --flake /etc/nixos#{} --print-build-logs",
    config_name
);
```

Modified legacy-channel command:
```rust
"stdbuf -oL -eL nix-channel --update && \
 stdbuf -oL -eL nixos-rebuild switch --print-build-logs"
```

**3.2.2 Add `--print-build-logs` to `nixos-rebuild`**

Always pass `-L` / `--print-build-logs` to `nixos-rebuild switch` so that
nix build logs appear inline during the operation, rather than being silently
discarded.

**3.2.3 Non-NixOS path (direct command)**

For `nix profile upgrade .*` and `nix-env -u`, output is already captured via
`tokio::join!(stdout_task, stderr_task)`. Since these run without `PrivilegedShell`,
buffering is the same issue. Use `stdbuf -oL -eL` as a wrapper here too if
`stdbuf` is available, but it is less critical because these commands are
typically fast.

---

## 4. Files That Need Modification

| File | Change |
|------|--------|
| `src/ui/window.rs` | Replace three-channel architecture with single `BackendEvent` enum + channel; rewrite event processing loop |
| `src/runner.rs` | Change `CommandRunner.tx` type to `async_channel::Sender<BackendEvent>`; update all `tx.send(...)` calls; update `PrivilegedShell::run_command` signature |
| `src/backends/mod.rs` | Add `BackendEvent` enum (or define it in `src/runner.rs` and import it here — choose one canonical location); update `Backend` trait comment if needed |
| `src/backends/nix.rs` | Add `--print-build-logs` to nixos-rebuild; add `stdbuf -oL -eL` prefix to nix/nixos-rebuild/nix-channel commands |

> `src/backends/flatpak.rs`, `src/backends/os_package_manager.rs`,
> `src/backends/homebrew.rs`, `src/ui/upgrade_page.rs` — **no changes
> needed** unless they also call `CommandRunner::run` (they do not
> construct runners; runners are created in `window.rs` and passed in via
> `run_update`).

---

## 5. Implementation Steps

### Step 1 — Define `BackendEvent` in `src/runner.rs`

Add the enum near the top of the file (after the existing `use` imports):

```rust
use crate::backends::{BackendKind, UpdateResult};

/// Unified event type used to stream all backend activity through a single
/// ordered channel to the GTK main thread.
#[derive(Debug)]
pub enum BackendEvent {
    /// The named backend has started its update operation.
    Started(BackendKind),
    /// A single line of log output produced by the named backend.
    LogLine(BackendKind, String),
    /// The named backend has finished; carries its result.
    Finished(BackendKind, UpdateResult),
}
```

### Step 2 — Update `CommandRunner` to use `BackendEvent`

Change the `tx` field type:
```rust
pub struct CommandRunner {
    tx: async_channel::Sender<BackendEvent>,
    kind: BackendKind,
    shell: Option<Arc<Mutex<PrivilegedShell>>>,
}
```

Update `CommandRunner::new` accordingly.

Update `CommandRunner::send` (the private helper):
```rust
async fn send(&self, msg: String) {
    let _ = self.tx.send(BackendEvent::LogLine(self.kind, msg)).await;
}
```

Update the stdout/stderr send calls inside the direct-command path:
```rust
let _ = tx_stdout.send(BackendEvent::LogLine(kind_stdout, line)).await;
// and
let _ = tx_stderr.send(BackendEvent::LogLine(kind_stderr, line)).await;
```

### Step 3 — Update `PrivilegedShell::run_command`

Change the `tx` parameter type from
`&async_channel::Sender<(BackendKind, String)>` to
`&async_channel::Sender<BackendEvent>`:

```rust
pub async fn run_command(
    &mut self,
    args: &[&str],
    tx: &async_channel::Sender<BackendEvent>,
    kind: BackendKind,
) -> Result<String, String> {
    // ...
    let _ = tx.send(BackendEvent::LogLine(kind, content)).await;
    // ...
}
```

### Step 4 — Rewrite the update worker in `src/ui/window.rs`

Replace the three-channel setup (lines 266–269) with a single channel:

```rust
let (event_tx, event_rx) = async_channel::unbounded::<BackendEvent>();
let (auth_status_tx, auth_status_rx) = async_channel::bounded::<Result<(), String>>(1);

let event_tx_thread = event_tx.clone();
```

Inside the worker closure:
```rust
for backend in &ordered_backends {
    let kind = backend.kind();
    let _ = event_tx_thread.send(BackendEvent::Started(kind)).await;
    let runner = CommandRunner::new(event_tx_thread.clone(), kind, shell.clone());
    let result = backend.run_update(&runner).await;
    let _ = event_tx_thread.send(BackendEvent::Finished(kind, result)).await;
}
```

After the worker spawn, drop the original sender:
```rust
drop(event_tx);
```

After auth completes, replace the three separate futures + loop with a single
unified event loop:

```rust
status_ref.set_label("Updating\u{2026}");

let mut has_error = false;
let mut self_updated = false;
while let Ok(event) = event_rx.recv().await {
    match event {
        BackendEvent::Started(kind) => {
            let rows_borrowed = rows_ref.borrow();
            if let Some((_, row)) = rows_borrowed.iter().find(|(k, _)| *k == kind) {
                row.set_status_running();
            }
        }
        BackendEvent::LogLine(kind, line) => {
            log_ref.append_line(&format!("[{kind}] {line}"));
        }
        BackendEvent::Finished(kind, result) => {
            let rows_borrowed = rows_ref.borrow();
            if let Some((_, row)) = rows_borrowed.iter().find(|(k, _)| *k == kind) {
                match &result {
                    UpdateResult::Success { updated_count } => {
                        row.set_status_success(*updated_count);
                    }
                    UpdateResult::SuccessWithSelfUpdate { updated_count } => {
                        row.set_status_success(*updated_count);
                        self_updated = true;
                    }
                    UpdateResult::Error(msg) => {
                        row.set_status_error(msg);
                        has_error = true;
                    }
                    UpdateResult::Skipped(msg) => {
                        row.set_status_skipped(msg);
                    }
                }
            }
        }
    }
}
```

Remove the two separate `glib::spawn_future_local` calls for `started_rx` and
`rx` entirely — they are replaced by the unified loop above.

### Step 5 — Update `src/backends/nix.rs` — Add `stdbuf` and `--print-build-logs`

**Flake-based NixOS** (`run_update`, around line 294):

```rust
let cmd = format!(
    "stdbuf -oL -eL \
     nix --extra-experimental-features 'nix-command flakes' \
     flake update --flake /etc/nixos && \
     stdbuf -oL -eL \
     nixos-rebuild switch --flake /etc/nixos#{} --print-build-logs",
    config_name
);
```

**Legacy-channel NixOS** (`run_update`, around line 318):

```rust
runner.run(
    "pkexec",
    &[
        "env",
        "PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin",
        "sh",
        "-c",
        "stdbuf -oL -eL nix-channel --update && \
         stdbuf -oL -eL nixos-rebuild switch --print-build-logs",
    ],
).await
```

---

## 6. Risks and Mitigations

| Risk | Mitigation |
|------|-----------|
| `stdbuf` not available on non-NixOS systems (for non-NixOS Nix path) | Guard with `which::which("stdbuf").is_ok()`; fall back to running without it |
| `stdbuf` does not affect Rust-native I/O in nix (only libc stdio) | `nix` and `nixos-rebuild` use C++ stdio; `stdbuf` applies correctly. Confirmed: NixOS `nix` binaries are linked against glibc |
| `BackendEvent` enum changes `CommandRunner` public API | `CommandRunner` is only constructed in `window.rs`; all backends receive it by reference and call `.run()`. No external API breakage |
| Unified loop removes `glib::spawn_future_local` for log/started | These were spawned to allow interleaving. The unified loop processes all events cooperatively within the same `glib::spawn_future_local` context and is equivalent in GTK responsiveness |
| `--print-build-logs` produces extremely verbose output | This is desirable — it was the missing output. Users can collapse the log panel if they don't want to see it |

---

## 7. Summary

Two independent bugs exist:

**Bug 1 (Parallel appearance):** Three separate async channels with three
independent futures on the GTK main thread create a race condition. The
`started_rx` future can process `Started(Flatpak)` before the result loop
processes `Finished(Nix)`, making both rows appear as "running" simultaneously.
**Fix:** Unify into a single `BackendEvent` channel processed by a single loop.

**Bug 2 (No Nix output):** `nixos-rebuild switch` runs inside the
`PrivilegedShell` pipe context, causing full stdio buffering. No output
appears in the log panel until the command exits. Additionally,
`--print-build-logs` is omitted, so nix build logs are never emitted.
**Fix:** Prefix nix commands with `stdbuf -oL -eL`; add `--print-build-logs`
to all `nixos-rebuild switch` invocations.

**Files to modify:** `src/ui/window.rs`, `src/runner.rs`, `src/backends/nix.rs`
