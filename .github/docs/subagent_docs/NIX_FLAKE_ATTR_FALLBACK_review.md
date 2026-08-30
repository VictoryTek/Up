# NIX_FLAKE_ATTR_FALLBACK — Review

Scope reviewed: `src/backends/nix.rs` (only modified file).
Spec: `.github/docs/subagent_docs/NIX_FLAKE_ATTR_FALLBACK_spec.md`.

## Findings

- **Spec compliance** — implemented exactly as specified: variant file first
  (now optional, empty tolerated), then `nix eval` auto-detect with
  single/multi/host-match/none branches. Signature unchanged; all three call
  sites (`run_update`, `run_selected_update`, `upgrade/execute.rs`) unaffected.
- **Security** — every returned name still passes `validate_flake_attr()`
  before interpolation, including the auto-detected and hostname branches. No
  new shell surface. `nix eval` args are a fixed array (no interpolation).
- **Consistency** — flatpak-sandbox routing mirrors the existing `is_*`
  helpers; error strings match the module's existing style.
- **Behaviour preservation** — VexOS path (variant file present) returns the
  file contents verbatim, identical to before. Existing tests unchanged and
  green.
- **Testability** — JSON parsing split into pure `parse_configuration_names()`
  with 6 unit tests (single, multi, empty, malformed, non-array, non-string).
  The process-spawning `nixos_configuration_names()` is not unit-tested
  (consistent with the other `is_*` probes in this file, which the existing
  test comments already acknowledge as needing a future `SystemProber`).
- **No dead code / no unrelated changes.**

## Build validation (via `nix develop`)

| Command | Result |
|---|---|
| `cargo fmt --check` | pass |
| `cargo clippy -- -D warnings` | pass (0 warnings) |
| `cargo build` | pass |
| `cargo test` | 132 passed; 0 failed |

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 100% | A |
| Best Practices | 95% | A |
| Functionality | 95% | A |
| Code Quality | 95% | A |
| Security | 100% | A |
| Performance | 95% | A |
| Consistency | 100% | A |
| Build Success | 100% | A |

**Overall Grade: A (97%)**

## Result

PASS
