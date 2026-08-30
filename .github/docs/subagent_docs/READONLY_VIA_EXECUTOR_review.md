# READONLY_VIA_EXECUTOR — Review

Spec: `.github/docs/subagent_docs/READONLY_VIA_EXECUTOR_spec.md`
Scope: MASTER_PLAN item 11 — "list_available + estimate_size" slice.

## Modified files

- `src/executor.rs` — `ProbeOutput`, `CommandExecutor::probe`, `spawn_probe`,
  `SystemExecutor`, `MockExecutor::probe` + `with_probe`.
- `src/runner.rs` — `CommandRunner::probe` (delegates to `spawn_probe`).
- `src/backends/mod.rs` — `list_available` / `estimate_size` / `count_available`
  now take `runner: &dyn CommandExecutor`.
- `src/backends/{os_package_manager,flatpak,fwupd,homebrew,nix}.rs`,
  `src/plugins/backend.rs` — read-only bodies migrated to `runner.probe(...)`.
- `src/check.rs`, `src/ui/window.rs` — call sites construct `SystemExecutor`.

## Findings

- **Spec compliance** — matches. Every migrated site preserves its original
  exit-code handling (dnf `Some(1)` vs `100`, fwupd `code == 2`, zypper
  status-agnostic, flatpak per-scope tolerance).
- **Deliberate behaviour change** — APT `estimate_size` no longer sets
  `DEBIAN_FRONTEND=noninteractive`. `apt-get -s upgrade` only simulates and
  never configures packages, so it cannot prompt; `LANG`/`LC_ALL=C` retained
  for parse stability. Documented inline.
- **Boundary left explicit** — `nix.rs::nixos_flake_tempdir_check` keeps its
  direct `tokio::process::Command` (needs `.current_dir()` + filesystem
  copies); commented. All other read-only spawns in the three target methods
  now route through `probe`.
- **Testability achieved** — 13 new `MockExecutor`-driven tests covering
  APT/DNF/pacman/zypper/flatpak/fwupd/homebrew/plugin `list_available`, APT
  `estimate_size`, `count_available` delegation, and exit-code edge cases
  (dnf 1/100, fwupd 2, flatpak partial failure).
- **Two probe impls identical** — `SystemExecutor::probe` and
  `CommandRunner::probe` both call the shared `spawn_probe`.
- **No unintended call sites** — `list_available`/`estimate_size`/
  `count_available` have exactly the two known callers; both updated.
- **Out of scope, untouched** — sync detection probes, nix update-path
  streaming, `run_cleanup` internal reads.

## Build validation (`nix develop`)

| Command | Result |
|---|---|
| `cargo fmt --check` | pass |
| `cargo clippy -- -D warnings` | pass (0 warnings) |
| `cargo build` | pass |
| `cargo test` | 145 passed / 0 failed (was 132) |
| `scripts/preflight.sh` | exit 0 |

Note: `cargo clippy --all-targets` surfaces pre-existing test-only lints
(`nix.rs` MutexGuard-across-await in HOME-env tests, `progress.rs` `.last()`);
neither CI (`.gitlab-ci.yml:45`) nor preflight passes `--all-targets`, so these
are not gating and are not touched by this change.

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 100% | A |
| Best Practices | 95% | A |
| Functionality | 95% | A |
| Code Quality | 93% | A |
| Security | 100% | A |
| Performance | 97% | A |
| Consistency | 95% | A |
| Build Success | 100% | A |

**Overall Grade: A (97%)**

## Result

PASS
