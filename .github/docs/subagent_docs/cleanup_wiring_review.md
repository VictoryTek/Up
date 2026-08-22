# Review: Wire up Cleanup / maintenance mode (item 8)

Spec: `.github/docs/subagent_docs/cleanup_wiring_spec.md`

Modified files:
- `src/ui/window.rs`
- `src/orchestrator.rs` (removed `#[allow(dead_code)]` from `CleanupOrchestrator`)

## Findings

1. **Specification Compliance** — Implementation matches the spec:
   `UpdatePageResult` extended with `run_cleanup: Rc<dyn Fn()>`, a
   `win.cleanup` action registered and added to the overflow menu above
   "About Up", the closure guards on `updating`/empty-cleanup-set, and
   `spawn_cleanup` streams `OrchestratorEvent`s into `log_panel`/
   `status_label` only (deliberately not touching per-row `UpdateRow`
   state, per the spec's reasoning about avoiding "N updated" vs "N
   removed" semantic mismatch).
2. **Best Practices** — Reuses the existing `updating` flag for mutual
   exclusion instead of introducing new synchronization state; follows
   the established `run_checks`/`spawn_cache_bypass` patterns already in
   this file rather than inventing a new structure.
3. **Consistency** — `win.cleanup` action registration mirrors
   `win.about` exactly; `spawn_cleanup`'s event-loop shape mirrors
   `spawn_cache_bypass`'s (same `AuthStarted`/`AuthSucceeded`/
   `AuthFailed`/`BackendStarted`/`BackendLog`/`BackendFinished`/
   `AllFinished` handling), differing only in *what* it updates
   (log/status only vs. also per-row UI), which is the correct
   distinction it should make.
4. **Maintainability** — `#[allow(dead_code)]` removed from
   `CleanupOrchestrator` (struct + impl) now that it has a real caller.
5. **Completeness** — All backends' `supports_cleanup()`/`run_cleanup()`
   are already implemented (verified in Phase 1); nothing else needed
   wiring. `updating`, `update_button`, `log_panel`, `status_label`,
   `detected` were all already in scope in `build_update_page()` — no
   new shared state introduced.
6. **Performance** — No regression; cleanup runs on the same background
   Tokio runtime as every other orchestrator operation.
7. **Security** — No new attack surface; `CleanupOrchestrator` reuses the
   exact same `PrivilegedShell`/`CommandRunner`/polkit-auth pipeline
   already used and reviewed for `UpdateOrchestrator`.
8. **API Currency** — N/A.
9. **Build Validation:**

   `cargo build`:
   ```
   Compiling up v2.2.0 (/home/nimda/Projects/Up)
   Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.65-5.73s
   ```
   Clean — confirms removing `#[allow(dead_code)]` surfaced no dead-code
   warnings.

   `cargo clippy -- -D warnings`: clean, no warnings (including after
   `cargo fmt` collapsed the `BackendFinished` match arm's body into a
   single-expression match per clippy's `single_match_else`-adjacent
   style preference, applied automatically by `cargo fmt`, not a
   semantic change).

   `cargo fmt --check`: clean, no diff, after one `cargo fmt` pass.

   `cargo test`:
   ```
   test result: ok. 109 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
   ```
   (unchanged — this is UI-glue wiring with no new pure-logic branch,
   consistent with the existing test-coverage boundary between
   backend/storage logic and window-layer wiring, same as items 6 and 8's
   surrounding code).

   **Manual functional check:** launched `./target/debug/up` — process
   started and ran for 5 seconds with no crash or error output before
   being stopped by `timeout`. Live interactive verification (opening the
   overflow menu, clicking "Clean Up", observing log output) was not
   performed — same screenshot/input-injection tooling limitations noted
   in the item 6 and item 7 reviews apply here (no working screen-capture
   or input-injection path in this sandboxed session).

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 100% | A |
| Best Practices | 100% | A |
| Functionality | 90% | A- (build/lint/test green and clean process launch confirmed; live menu-click interaction not verified in this environment) |
| Code Quality | 100% | A |
| Security | 100% | A |
| Performance | 100% | A |
| Consistency | 100% | A |
| Build Success | 100% | A |

**Overall Grade: A (99%)**

## Result: PASS
