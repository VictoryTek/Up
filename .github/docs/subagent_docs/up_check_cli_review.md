# Review: Wire up `up --check` CLI argument handling

Spec: `.github/docs/subagent_docs/up_check_cli_spec.md`

Modified files:
- `src/main.rs`
- `src/check.rs`

## Findings

1. **Specification Compliance** — Implementation matches the spec exactly:
   single `env_logger::init()` call retained in `main()`, `--check` branch
   added before GResource registration, duplicate `env_logger::init()`
   removed from `check.rs`, `#![allow(dead_code)]` removed from
   `check.rs`.
2. **Best Practices** — Idiomatic use of `std::env::args().any(...)`;
   matches existing project style (no external arg-parsing crate pulled
   in for one flag, consistent with "minimum code" principle).
3. **Consistency** — `gtk::glib::ExitCode::SUCCESS` used for the early
   return, matching the function's existing return type from `app.run()`.
4. **Maintainability** — No new abstractions; change is minimal and
   localized to `main()`'s dispatch logic.
5. **Completeness** — `up --check` now runs `check::run_check()` and exits
   without invoking GTK, matching what
   `data/io.github.up-check.service.in`'s `ExecStart=@BINDIR@/up --check`
   expects. No packaging changes were needed (verified `.service.in` /
   `.timer` already correct).
6. **Performance** — GResource registration is now skipped entirely on
   the check path, avoiding unnecessary GTK resource loading for a
   headless systemd oneshot invocation.
7. **Security** — No new attack surface; `check.rs` already validated
   (spawns `notify-send` with fixed argv, no shell interpolation).
8. **API Currency** — N/A, no external library integration.
9. **Build Validation:**

   `cargo build` (via `nix develop --command cargo build`):
   ```
   Compiling up v2.2.0 (/home/nimda/Projects/Up)
   Finished `dev` profile [unoptimized + debuginfo] target(s) in 7.24s
   ```

   `cargo clippy -- -D warnings`:
   ```
   Checking up v2.2.0 (/home/nimda/Projects/Up)
   Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.04s
   ```
   No warnings — confirms removing `#![allow(dead_code)]` did not surface
   any dead-code lint, since `run_check()` now exercises every private
   helper in the module.

   `cargo run -- --check` (manual functional check):
   ```
   Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.14s
   Running `target/debug/up --check`
   ```
   Exit code 0. No GTK/GApplication argument-parsing error — confirms the
   original bug (unknown-option rejection) is resolved.

   `cargo test`:
   ```
   test result: ok. 106 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
   ```

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

## Result: PASS
