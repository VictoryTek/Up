# VexOS Cache-Block Dialog — Review

**Spec:** `.github/docs/subagent_docs/vexos_cache_block_dialog_spec.md`
**Modified/added files:**
- `src/backends/nix.rs`
- `src/orchestrator.rs`
- `src/ui/cache_block_dialog.rs` (new)
- `src/ui/mod.rs`
- `src/ui/window.rs`

**Date:** 2026-07-09

---

## 1. Specification Compliance

Implementation follows the spec section-by-section:
- `CacheBypassMode` + `run_cache_bypass` in `nix.rs` (3.1) — implemented as specified.
- `extract_cache_block_message` in `nix.rs` (3.2) — implemented, with unit tests
  using the exact example output from the bug report.
- `run_cache_bypass` free function in `orchestrator.rs` (3.3) — implemented,
  reusing `PrivilegedShell`/`CommandRunner`/`BackendEvent`/`OrchestratorEvent`
  with no new channel types.
- `show_cache_block_dialog` in new `src/ui/cache_block_dialog.rs` (3.4) —
  implemented; "Wait" closes the toplevel window via `parent.root()` →
  downcast to `gtk::Window` → `.close()`, matching the user's confirmed
  decision that "Wait" quits Up.
- `window.rs` wiring at both `CacheMiss` call sites (3.5) — implemented; the
  bypass-execution logic is factored into one shared `spawn_cache_bypass`
  function rather than duplicated, as the spec required.

No deviations from the spec.

## 2. Best Practices / Consistency

- Follows the existing `reboot_dialog.rs` pattern for a one-off
  `adw::AlertDialog` with typed responses.
- Reuses `count_nix_store_operations` rather than writing a new output
  parser for the bypass commands.
- `pkexec env PATH=... sh -c "cd /etc/nixos && ..."` mirrors the existing
  `vexos-update` invocation exactly (same PATH restoration rationale).
- No new public API surface beyond what's needed: `CacheBypassMode` and
  `run_cache_bypass` in `nix.rs` are `pub(crate)`, matching the crate's
  existing convention for VexOS-only internals (`resolve_nixos_flake_attr`,
  `validate_flake_attr` are also `pub(crate)`).

## 3. Correctness Issue Found and Fixed During Review

**Issue:** An initial revision added an `updating: Rc<Cell<bool>>` guard to
`spawn_cache_bypass`, intending to prevent a per-row retry from racing with
a bypass run. This was incorrect: the shared `updating` flag is still
`true` for the *entire* main-run loop (it is only cleared on
`AllFinished`), and `CacheMiss` — hence the dialog — fires *during* that
loop, while other backends may still be running. Gating bypass-start on
`!updating.get()` would have silently no-op'd "Just Deploy"/"Just Update
All" whenever there were backends after Nix in the run order. Worse,
clearing `updating` when the bypass finished (potentially before the main
run's `AllFinished`) would have prematurely re-enabled retry buttons mid-run.

**Resolution:** Reverted; `spawn_cache_bypass` does not touch the shared
`updating` flag. `button.set_sensitive(false)`/`(true)` around the bypass
run is retained as the only guard, matching the granularity the rest of
the file already uses for the main Update-All button.

**Residual, accepted risk:** a per-row *retry* button click during an
active bypass run is not blocked by the bypass itself (only by the
existing `updating_retry.get()` check, which the bypass does not set).
This is a pre-existing class of risk (the codebase does not have a global
"any privileged operation in flight" lock) and is out of scope to fix here
per the "surgical changes" principle — flagging it rather than expanding
scope.

## 4. Completeness

- Both `CacheMiss` handling sites in `window.rs` (main run-all loop and the
  per-row retry loop) show the dialog with real captured log detail.
- `Wait` / `Just Deploy` / `Just Update All` are all wired to real behavior
  (no placeholder responses).
- Unit tests added for the one new pure function
  (`extract_cache_block_message`); the orchestrator/backend functions that
  shell out (`run_cache_bypass`) are not unit-testable without a
  `SystemProber`-style abstraction, consistent with the existing
  `run_update` VexOS branch's untested status (documented in `nix.rs`'s own
  test-module comment).

## 5. Performance / Security

- No new subprocess invocation patterns; reuses the existing shell-quoting-free
  `pkexec env PATH=... sh -c <fixed string>` pattern where the interpolated
  parts are a hardcoded recipe name (`"deploy"` / `"update-all"`, chosen
  from a closed Rust enum — not user input), so there is no injection
  surface introduced.
- No additional root sessions are held open longer than needed —
  `run_cache_bypass` closes its `PrivilegedShell` after the single command
  completes, same as `CleanupOrchestrator::run_all`.

## 6. Build Validation

All commands below were run inside `nix develop` (per
`scripts/preflight.sh`'s own re-exec behavior, since `pkg-config` is not on
PATH outside the dev shell) from the repository root.

```
$ cargo build
   Compiling up v2.0.4 (/home/nimda/Projects/Up)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.75s
```

```
$ cargo build -p up-daemon
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s
```

```
$ cargo test
test result: ok. 101 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```
(Includes 2 new tests: `extract_cache_block_message_none_when_absent`,
`extract_cache_block_message_extracts_and_strips_prefix`.)

```
$ cargo fmt --check
(no output — clean)
```

```
$ cargo clippy -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.81s
(no warnings)
```

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 100% | A |
| Best Practices | 95% | A |
| Functionality | 95% | A |
| Code Quality | 95% | A |
| Security | 100% | A |
| Performance | 100% | A |
| Consistency | 95% | A |
| Build Success | 100% | A |

**Overall Grade: A (97%)**

## Result: PASS

No CRITICAL issues outstanding. One correctness issue was found and fixed
during this review pass (the `updating`-flag race described in §3) before
build/test validation. One low-severity, explicitly-accepted residual risk
remains (per-row retry vs. bypass concurrency), documented above rather
than left silent.
