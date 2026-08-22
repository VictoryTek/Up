# Review: Fix plugin backends with `needs_root: true` running unprivileged (item 3)

Spec: `.github/docs/subagent_docs/plugin_privilege_fix_spec.md`

Modified files:
- `src/plugins/backend.rs`
- `src/executor.rs` (test-only `MockExecutor` call-recording, needed to
  write a regression test that actually verifies *which* program/args
  get invoked, since the existing test-suite convention only asserted on
  parsed output, never on the call itself)

## Findings

1. **Specification Compliance** — Implementation matches the spec: a
   `run_command` helper routes through `pkexec` (with an `env KEY=VALUE`
   prefix when `cmd.environment` is non-empty) whenever
   `descriptor.privilege.needs_root` is true, reusing the exact
   `pkexec env VAR=VAL ... program args` idiom already used in
   `src/backends/nix.rs`. Both `run_update` and `run_cleanup` now go
   through it; `list_available`/`estimate_size` untouched as specified.
2. **Best Practices** — Matches the established pattern for pkexec
   routing rather than introducing a third style (avoids compounding
   master plan item 32). No unwraps/panics added on the hot path.
3. **Consistency** — Uses the same `CommandExecutor` abstraction and
   `BackendError` propagation as every other backend.
4. **Maintainability** — Single new private helper eliminates the
   duplicated bug between `run_update`/`run_cleanup`; well-commented on
   *why* (routes needs-root plugins through pkexec).
5. **Completeness** — Fixes both privileged call sites
   (`run_update`, `run_cleanup`). Verified via a real regression test
   (see below) that a `needs_root: true` descriptor now calls
   `pkexec` (previously called the raw program directly — the exact bug)
   and that a non-root descriptor's behavior is unchanged.
6. **Performance** — No regression; identical command-execution path
   otherwise, just correct `pkexec`/`env` prefixing.
7. **Security** — Confirmed via `src/plugins/validate.rs` that: (a) only
   non-user-writable-path plugins may set `needs_root: true`, (b) command
   `args` are validated against shell metacharacters and path traversal
   at load time, (c) environment variable *keys* are restricted to a
   fixed allowlist. Since `PrivilegedShell::run_command`
   (`src/runner.rs:116-137`) treats every element of the `args` slice as
   a literal argv entry to the elevated shell (`shell_quote`-d, not
   shell-interpolated), and `pkexec env KEY=VAL ... program args` never
   re-parses these strings through a nested shell, there is no injection
   surface beyond what already existed for built-in backends using the
   same idiom.
8. **API Currency** — N/A.
9. **Build Validation:**

   `cargo build`:
   ```
   Compiling up v2.2.0 (/home/nimda/Projects/Up)
   Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.05-4.09s
   ```

   `cargo clippy -- -D warnings`: clean, no warnings.

   `cargo fmt --check`: clean after one `cargo fmt` pass on the new
   `MockExecutor` call-recording code (auto-formatted, no logic change).

   `cargo test` (full suite, including two new regression tests):
   ```
   test plugins::backend::tests::needs_root_update_routes_through_pkexec_with_env ... ok
   test plugins::backend::tests::non_root_update_runs_program_directly ... ok
   test result: ok. 108 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
   ```
   The first new test asserts the mock executor's recorded call for a
   `needs_root: true, environment: {LANG: C}` descriptor is exactly
   `("pkexec", ["env", "LANG=C", "testprog", "upgrade"])` — this is the
   test that would have failed against the pre-fix code (which called
   `("testprog", ["upgrade"])` directly, i.e. never routed through
   `pkexec`). The second test locks in that non-root plugin behavior is
   unchanged.

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
