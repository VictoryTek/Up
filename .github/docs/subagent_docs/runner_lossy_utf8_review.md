# Runner Lossy UTF-8 Decoding — Review

## Scope Reviewed

- `src/runner.rs` — new `read_line_lossy` helper; conversion of all four
  streamed read sites (handshake, `run_command` loop, `stdout_task`,
  `stderr_task`); new test module
- `src/upgrade/execute.rs` — dist-upgrade log tailer

## Findings

- **Spec compliance:** matches the spec. All five changes implemented; no read
  site in `src/runner.rs` still uses strict `read_line` / `lines()`.
- **Root cause fixed:** `read_until(b'\n', &mut Vec<u8>)` + `from_utf8_lossy`
  cannot return `InvalidData`; only genuine I/O errors surface. The
  `run_command` loop — the site that produced
  `Command failed (exit -1): Failed to read output: stream did not contain
  valid UTF-8` — now decodes lossily, so a stray byte in Nix build output no
  longer fails a successful update.
- **Behaviour preserved:**
  - `run_command` sentinel detection (`line.trim()`) and content trimming
    (`trim_end_matches('\n')`) unchanged — `read_line_lossy` keeps the newline,
    exactly like `read_line`.
  - EOF path (`None`) still runs the NixOS-activation-success heuristic and the
    "closed unexpectedly" error.
  - Direct path: `next_line()` stripped the terminator, so callers now
    `trim_end_matches(['\n', '\r'])`; `Err` still ends the read like the old
    `while let Ok(Some(_))`.
  - Final unterminated line is still delivered (`read_until` returns `n > 0`);
    covered by `returns_final_unterminated_line`.
- **Simplicity / surgical:** one 10-line helper removes duplication across four
  sites; no new dependencies; no public API change; `upgrade/execute.rs` keeps
  its `Ok(0)` / `Err` arm structure.
- **Consistency:** `String::from_utf8_lossy` is already the decoding idiom
  everywhere else in the crate (`executor.rs`, `changelog.rs`, `nix.rs`, …);
  this aligns the streaming reads with it.
- **Security:** no change to argument handling, shell quoting, or the sentinel
  protocol. Replacement chars cannot forge the per-session exit-code sentinel
  (printed by `printf` on its own line).
- **Performance:** one `Vec<u8>` allocation per line (same order as the previous
  `String::new()` per line); negligible.
- **i18n:** no user-visible strings added.

## Build Validation (`scripts/preflight.sh` via `nix develop`)

```
cargo fmt --check ............ pass (after cargo fmt)
cargo clippy -D warnings ..... pass
cargo build (+ up-daemon) .... pass
cargo test .................. 165 passed; 0 failed
nix flake check ............. pass
desktop-file-validate / appstreamcli / cargo-audit — tools absent, skipped (CI covers)
All preflight checks passed.
```

New tests (all pass):
`runner::tests::{decodes_valid_lines_keeping_newline,
replaces_invalid_utf8_instead_of_erroring, returns_final_unterminated_line}`.

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 100% | A |
| Best Practices | 100% | A |
| Functionality | 100% | A |
| Code Quality | 100% | A |
| Security | 100% | A |
| Performance | 100% | A |
| Consistency | 100% | A |
| Build Success | 100% | A |

**Overall Grade: A (100%)**

## Result

**PASS** — no refinement required. Preflight passed.
