# ERROR_TAIL_DIALOG — Review

Spec: `.github/docs/subagent_docs/ERROR_TAIL_DIALOG_spec.md`
Scope: MASTER_PLAN item 24.

## Modified files

- `src/runner.rs` — `PrivilegedShell::run_command` and `CommandRunner::run`
  append the retained output tail to the non-zero-exit error message.
- `src/backends/mod.rs` — `BackendError::from_string` classifies on the first
  line only; 2 new tests.
- `src/ui/update_row.rs` — `error_button` + `error_details`; `set_status_error`
  shows a one-line label + a details button; other setters hide it; new
  `show_error_details_dialog`.

## Findings

- **Feature delivered** — a failed update now shows a compact label plus a
  details button that opens a scrollable dialog with the full 100-line tail.
- **Classifier hardened** — feeding command output into `from_string` no
  longer risks an `Exit`→`Spawn`/`AuthCancelled` misclassification (tests
  cover the "No such file or directory" tail case and the still-working
  auth-cancel / genuine-spawn cases). This is a small, safe step toward
  item 13 part B without doing the full typed-error refactor.
- Button visibility is consistent across all status transitions.
- `fwupd` code-2 / `nix` code-2 special cases unaffected (those errors carry
  no tail).
- Dialog reuses the `show_changelog_dialog` layout pattern (monospace
  TextView + ScrolledWindow).

## Build validation (`nix develop`)

| Command | Result |
|---|---|
| `cargo fmt --check` | pass |
| `cargo clippy -- -D warnings` | pass (0 warnings) |
| `cargo build` | pass |
| `cargo test` | 159 passed / 0 failed (was 157; +2) |
| `scripts/preflight.sh` | exit 0 |

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 100% | A |
| Best Practices | 97% | A |
| Functionality | 95% | A |
| Code Quality | 96% | A |
| Security | 100% | A |
| Performance | 100% | A |
| Consistency | 97% | A |
| Build Success | 100% | A |

**Overall Grade: A (98%)**

## Result

PASS
