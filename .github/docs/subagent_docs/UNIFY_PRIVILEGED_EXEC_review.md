# UNIFY_PRIVILEGED_EXEC — Review

Spec: `.github/docs/subagent_docs/UNIFY_PRIVILEGED_EXEC_spec.md`
Scope: MASTER_PLAN item 12 — full async + PrivilegedShell unification.

## Modified files

- `src/upgrade/execute.rs` — `execute_upgrade` is now `async` and takes
  `&dyn CommandExecutor`; every step calls `run_step()` →
  `runner.run("pkexec", …)`. New `run_upgrade()` entry point owns the
  `PrivilegedShell` + `CommandRunner` + log forwarder lifecycle
  (mirrors `orchestrator::run_cache_bypass`). New `upgrade_kind()` maps
  distro id → `BackendKind` for log tagging.
- `src/runner.rs` — `run_command_sync` deleted (was used only by `execute.rs`).
- `src/upgrade/mod.rs` — re-exports `run_upgrade` instead of `execute_upgrade`.
- `src/ui/upgrade_page.rs` — the upgrade `std::thread::spawn` + plain-string
  bridge is replaced by `crate::ui::spawn_background_async` calling
  `run_upgrade`.

## Findings

- **One polkit prompt per upgrade.** All `pkexec` steps route through the
  single pre-authenticated `PrivilegedShell`. Fedora drops from up to 4
  prompts to 1 (plugin-install + download + reboot all share the shell);
  legacy-NixOS and flake-NixOS drop from 2 to 1.
- **Command args unchanged** — every step passes byte-for-byte identical argv;
  only the transport changed. Step ordering and narrative strings preserved.
- **Fedora reboot** now runs through the shell; a non-`Ok` result is ignored
  because `dnf system-upgrade reboot` SIGTERMs the process. Net effect matches
  the previous fire-and-forget semantics with no extra prompt.
- **Legacy-NixOS rebuild** gains the `is_nixos_activation_success` fallback
  for free (previously `run_command_sync` returned `false` when activation
  killed the child → spurious "failed" even on success).
- **Cosmetic log change** — the old `run_command_sync` emitted
  "Command completed successfully." / "Command exited with code N" trailer
  lines; the shared runner emits a leading `$ <cmd>` and streams output
  without a trailer. Acceptable / cleaner.
- **Duplication removed** — `run_command_sync` (a blocking re-implementation of
  `CommandRunner::run`'s pipe draining) is gone; one execution stack remains.
- **No `std::process`/`pkexec` spawn left in `execute.rs`** except the passive
  Ubuntu `/var/log/dist-upgrade/main.log` tail follower (a file reader, not a
  command runner) and `detect_next_fedora_version`'s `rpm -E` probe.
- **Tests** — module test converted to `#[tokio::test]`, still asserts the
  unsupported-distro `Err` and now also asserts the runner was never touched
  (via `MockExecutor`). Added `upgrade_kind` mapping test.

## Build validation (`nix develop`)

| Command | Result |
|---|---|
| `cargo fmt --check` | pass |
| `cargo clippy -- -D warnings` | pass (0 warnings) |
| `cargo build` | pass |
| `cargo test` | 146 passed / 0 failed |
| `scripts/preflight.sh` | exit 0 |

## Residual risk

The distro-upgrade paths have no integration tests and cannot be exercised
without a real Ubuntu/Fedora/openSUSE/NixOS system. Mitigation was to keep
every command invocation identical and change only the execution transport.
Manual verification on a real system is advisable before a release that ships
this.

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 100% | A |
| Best Practices | 95% | A |
| Functionality | 92% | A |
| Code Quality | 95% | A |
| Security | 100% | A |
| Performance | 95% | A |
| Consistency | 98% | A |
| Build Success | 100% | A |

**Overall Grade: A (96%)**

## Result

PASS
