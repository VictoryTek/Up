# SHARED_BACKEND_FINISHED — Review

Spec: `.github/docs/subagent_docs/SHARED_BACKEND_FINISHED_spec.md`
Scope: MASTER_PLAN item 16.

## Modified files

- `src/ui/window.rs` — new `BackendFinishedOutcome` + `apply_backend_finished`
  helper. The "Update All" loop and the per-row retry loop both call it for
  `BackendFinished`; ~170 lines of duplicated match/dialog/history code
  replaced by two call sites + the ~105-line helper. Retry closure now
  captures `restart_banner` (via `#[weak] restart_banner` on the
  detect-completion closure + a per-row `restart_banner_retry` clone).

## Findings

- **Single source of truth** — a new `UpdateResult` variant now needs handling
  only in `apply_backend_finished`.
- **Drift fixed** — the retry path reveals the restart banner on
  `SuccessWithSelfUpdate` (previously it silently ignored it).
- **Main-loop behaviour preserved** — helper body is the former inline code;
  `has_error` / `self_updated` are now `|=`-folded from the return value. The
  progress-bar segment math and the cancel-handle/cancel-button teardown are
  untouched.
- **Left as-is (out of scope, legitimately different)** — the cleanup loop
  (log-only) and cache-bypass loop (no dialog/history); the retry loop's
  absent progress bar / cancel button (single-backend quick action).
- `nix_log_lines` is still accumulated in both loops and passed to the helper
  for the cache-block message extraction.

## Build validation (`nix develop`)

| Command | Result |
|---|---|
| `cargo fmt --check` | pass |
| `cargo clippy -- -D warnings` | pass (0 warnings) |
| `cargo build` | pass |
| `cargo test` | 149 passed / 0 failed |
| `scripts/preflight.sh` | exit 0 |

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 100% | A |
| Best Practices | 95% | A |
| Functionality | 95% | A |
| Code Quality | 96% | A |
| Security | 100% | A |
| Performance | 100% | A |
| Consistency | 97% | A |
| Build Success | 100% | A |

**Overall Grade: A (97%)**

## Result

PASS
