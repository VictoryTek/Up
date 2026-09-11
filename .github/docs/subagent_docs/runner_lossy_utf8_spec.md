# Runner Lossy UTF-8 Decoding — Specification

## Current State Analysis

`src/runner.rs` streams subprocess output back to the UI line by line. Every
read site decodes the byte stream as **strict UTF-8**:

| Site | Code | Failure mode on invalid UTF-8 |
|------|------|-------------------------------|
| `PrivilegedShell::new` (`runner.rs:88`) | `reader.read_line(&mut String)` | handshake only — reads a fixed ASCII sentinel, not a risk |
| `PrivilegedShell::run_command` loop (`runner.rs:163-168`) | `reader.read_line(&mut String)?` | **`?` propagates the error → the read loop aborts, the pkexec shell is torn down, the command is reported `Command failed (exit -1): Failed to read output: stream did not contain valid UTF-8`** |
| `CommandRunner::run` direct path (`runner.rs:334-335`, `360-361`) | `BufReader::new(pipe).lines()` + `while let Ok(Some(line))` | `Err` arm falls through the `while let` → reading stops silently, the rest of the log and the exit-code sentinel are lost |
| `upgrade/execute.rs:124` | std `BufReader::read_line` in the dist-upgrade log tailer | `Err(_) => break` → log tail stops silently mid-upgrade |

`tokio::io::AsyncBufReadExt::read_line` (and `read_to_string`, `lines`) return
`io::ErrorKind::InvalidData` ("stream did not contain valid UTF-8") when the
bytes read are not valid UTF-8.

## Problem Definition

Nix / `nixos-rebuild` / `vexos-update` build-log output is **not guaranteed to
be valid UTF-8**: progress-bar redraws, fetched store-path / tarball names,
compiler diagnostics emitted under a non-UTF-8 locale, and raw fetcher output
can all contain stray bytes. A single such byte anywhere in the merged
`stdout`+`stderr` stream makes `read_line` fail, and Up reports the whole update
as failed with exit `-1` **even when the privileged command itself succeeded**.

Confirmed from the user's history log
(`~/.local/share/up/history.jsonl`):

```json
{"backend":"Nix","result":"error",
 "error":"Command failed (exit -1): Failed to read output: stream did not contain valid UTF-8"}
```

Running `just update` (same `vexos-update` script, output straight to a terminal
that does not validate UTF-8) succeeded immediately afterward — the system state
was never the problem.

## Proposed Solution

Decode every streamed line **lossily**: read raw bytes up to `\n` with
`AsyncBufReadExt::read_until(b'\n', &mut Vec<u8>)` (which never fails on invalid
UTF-8 — it only surfaces genuine I/O errors), then convert with
`String::from_utf8_lossy`. Invalid bytes become U+FFFD; the stream continues.

### Change 1 — `runner.rs` shared helper

Add a private async helper:

```rust
/// Read one `\n`-terminated chunk from `reader`, decoded lossily.
/// `Ok(None)` at EOF. Invalid UTF-8 bytes become U+FFFD instead of
/// aborting the stream — subprocess build output is not guaranteed UTF-8.
async fn read_line_lossy<R>(reader: &mut R) -> std::io::Result<Option<String>>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    let mut buf = Vec::new();
    if reader.read_until(b'\n', &mut buf).await? == 0 {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}
```

### Change 2 — `PrivilegedShell::run_command` loop (`runner.rs:163-215`)

Replace the `read_line` call with `read_line_lossy`. `Ok(None)` takes the
existing `n == 0` branch (unexpected-close / activation-success detection
unchanged). The returned `String` still carries its trailing `\n`, so the
existing `line.trim()` (sentinel match) and `line.trim_end_matches('\n')`
(content) logic is unchanged.

### Change 3 — `CommandRunner::run` direct path (`runner.rs:330-378`)

Replace `BufReader::new(pipe).lines()` / `while let Ok(Some(line))` in both the
`stdout_task` and `stderr_task` with a `loop` over `read_line_lossy`, trimming a
trailing `\n` / `\r` to match the old `next_line()` behaviour (which stripped
the terminator). Genuine I/O errors break the loop, same as the old `Err` arm.

### Change 4 — `PrivilegedShell::new` handshake (`runner.rs:88`)

Switch to `read_line_lossy` for consistency; behaviour is identical for the
ASCII sentinel. Low risk, keeps one decoding path.

### Change 5 — `upgrade/execute.rs:118-137`

The synchronous log tailer uses `std::io::BufRead::read_line`. Replace with
`read_until(b'\n', &mut Vec<u8>)` + `String::from_utf8_lossy`; the existing
`Ok(0)` / `Err` arms are preserved.

## Implementation Steps

1. Add `read_line_lossy` helper to `runner.rs` → verify: `cargo build`.
2. Convert the three `runner.rs` read sites → verify: `cargo build`,
   `cargo clippy -- -D warnings`.
3. Convert the `upgrade/execute.rs` tailer → verify: `cargo build`.
4. Add unit tests for `read_line_lossy` (valid input, invalid-UTF-8 bytes
   preserved as U+FFFD, EOF → `None`, no trailing newline) → verify:
   `cargo test`.
5. `scripts/preflight.sh` → verify: exit 0.

## Dependencies

None. `tokio` (with the `io-util` feature already enabled — `AsyncBufReadExt`
is in use) provides `read_until`. `String::from_utf8_lossy` is std. Context7
not required (no new dependency, no external API surface).

## Configuration Changes

None. No user-visible strings. No `po` changes.

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| `read_until` keeps the `\n`; `next_line()` stripped it | Helper callers trim trailing `\n`/`\r` where the old code relied on stripped lines (Change 3). |
| A line with no trailing newline at EOF is dropped | `read_until` returns the partial line with `n > 0`; only `n == 0` is EOF, so the final unterminated line is still delivered. |
| Very long line with no newline buffers unbounded | Pre-existing behaviour with `read_line` too; subprocess output is newline-oriented. No regression. |
| U+FFFD noise in the log panel | Acceptable and expected — a few replacement chars beat aborting a real update. `strip_ansi` in `log_panel.rs` is unaffected. |
| Sentinel line corrupted by a preceding invalid byte on the same line | The exit-code sentinel is printed by `printf` on its own line after `\n`; invalid bytes on prior lines cannot bleed into it. |
