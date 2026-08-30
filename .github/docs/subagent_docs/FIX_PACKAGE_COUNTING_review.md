# FIX_PACKAGE_COUNTING — Review

Spec: `.github/docs/subagent_docs/FIX_PACKAGE_COUNTING_spec.md`
Scope: MASTER_PLAN item 22 (BUGS M5 + M6).

## Modified files

- `src/upgrade/check.rs` — `check_packages_up_to_date` counts pending packages
  with the backend parsers (`parse_apt_list_upgradable` / `parse_dnf_list_upgrades`
  / `parse_zypper_list_updates`) instead of a raw line count; 2 new tests.
- `src/backends/os_package_manager.rs` — `LC_ALL=C` added to the APT
  `run_update` / `run_selected_update` shell commands; `count_apt_upgraded`
  rewritten (strict summary match + "Setting up" fallback); 3 new tests.

## Findings

- **M6 fixed** — the `dnf check-update` metadata header and section labels no
  longer count as packages; an up-to-date Fedora no longer fails the
  prerequisite check. Parser choice is a zero-capture closure coerced to
  `fn(&str) -> usize`.
- **M5 fixed** — localised / reworded summary lines and the
  `--only-upgrade` "0 upgraded" case now fall back to counting dpkg
  `Setting up` lines; a bare leading integer on an unrelated line is no longer
  mistaken for the count. `LC_ALL=C` keeps the common path on the fast summary
  parse.
- No behaviour change for a genuine no-op update (still `0`); the three
  pre-existing `count_apt_upgraded` tests pass unchanged.
- The M6 counters reuse already-tested parsers — no new parsing logic.

## Build validation (`nix develop`)

| Command | Result |
|---|---|
| `cargo fmt --check` | pass |
| `cargo clippy -- -D warnings` | pass (0 warnings) |
| `cargo build` | pass |
| `cargo test` | 157 passed / 0 failed (was 152; +5) |
| `scripts/preflight.sh` | exit 0 |

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 100% | A |
| Best Practices | 98% | A |
| Functionality | 96% | A |
| Code Quality | 97% | A |
| Security | 100% | A |
| Performance | 100% | A |
| Consistency | 98% | A |
| Build Success | 100% | A |

**Overall Grade: A (98%)**

## Result

PASS
