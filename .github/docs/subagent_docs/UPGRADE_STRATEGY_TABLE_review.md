# UPGRADE_STRATEGY_TABLE — Review

Spec: `.github/docs/subagent_docs/UPGRADE_STRATEGY_TABLE_spec.md`
Scope: MASTER_PLAN item 17.

## Modified files

- `src/upgrade/detect.rs` — new `UpgradeStrategy` enum + `for_distro`;
  `upgrade_supported` derived from it; the ID/ID_LIKE match and the orphaned
  `id_like` binding removed. New test module.
- `src/upgrade/execute.rs` — `execute_upgrade` and `upgrade_kind` dispatch on
  `UpgradeStrategy::for_distro`.
- `src/upgrade/version.rs` — `check_upgrade_available` dispatches on it.

## Findings

- **Three lists → one.** All dispatch points now derive from
  `UpgradeStrategy::for_distro`; they cannot drift.
- **Honest support surface.** debian / linuxmint / pop / elementary / zorin /
  rhel / centos / ID_LIKE matches are no longer reported as `upgrade_supported`
  — they had no implemented path and would fail at "Start Upgrade". The
  upgrade page now shows "not supported for this distribution yet" up front.
- **No behavioural regression for real upgrades** — `execute_upgrade`
  previously matched `id == "ubuntu"` exactly, so no derivative ever actually
  upgraded; this only removes the misleading UI affordance.
- Orphan cleanup: `id_like` binding removed (its only use was the deleted
  match).
- **Tests** — strategy-table mapping + a property test asserting
  `detect_distro().upgrade_supported == for_distro(id).is_some()` on the host.
- Item-12's `upgrade_kind("linuxmint")` test still passes (→ `None` → `Apt`
  fallback).

## Build validation (`nix develop`)

| Command | Result |
|---|---|
| `cargo fmt --check` | pass |
| `cargo clippy -- -D warnings` | pass (0 warnings) |
| `cargo build` | pass |
| `cargo test` | 151 passed / 0 failed (was 149) |
| `scripts/preflight.sh` | exit 0 |

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 100% | A |
| Best Practices | 100% | A |
| Functionality | 98% | A |
| Code Quality | 100% | A |
| Security | 100% | A |
| Performance | 100% | A |
| Consistency | 100% | A |
| Build Success | 100% | A |

**Overall Grade: A (99%)**

## Result

PASS
