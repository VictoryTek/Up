# Review: True Package-Level Progress Bar

**Spec:** `.github/docs/subagent_docs/true_progress_bar_spec.md`
**Result:** NEEDS_REFINEMENT (1 CRITICAL, 1 RECOMMENDED)

---

## 1. Files Reviewed

| File | Change |
|---|---|
| `src/progress.rs` | New — `ProgressTracker` + per-backend parsers + 16 unit tests |
| `src/main.rs` | `mod progress;` registered |
| `src/orchestrator.rs` | `OrchestratorEvent::BackendProgress`; tracker driven from the log-forwarding task |
| `src/ui/window.rs` | `BackendProgress` handler; `BackendStarted` sets segment floor; `is_apt_status_line` filter; no-op arm in the cache-bypass loop |
| `src/backends/os_package_manager.rs` | `-o APT::Status-Fd=1` added to both APT update commands |

## 2. Build & Test Validation

The developer host is Windows; `gtk4-sys` cannot configure (`pkg-config` absent), so
`cargo build` / `cargo test` for the `up` crate cannot run here — verified, not assumed:

```
cargo:warning=Could not run `PKG_CONFIG_ALLOW_SYSTEM_CFLAGS=1 pkg-config --libs --cflags gtk4 'gtk4 >= 4.10'`
The pkg-config command could not be found.
```

`src/progress.rs` has no GTK dependency, so it was compiled and tested verbatim in a standalone
crate with a stub `BackendKind`:

```
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
cargo clippy --all-targets -- -D warnings   → Finished, no warnings
rustfmt --check src/progress.rs src/orchestrator.rs src/ui/window.rs
        src/backends/os_package_manager.rs src/main.rs   → no diff
```

Two defects were found and fixed during this pass (both now covered by tests):
`split_ratio` failed on flatpak's `1/2…` (trailing ellipsis), and the throttle test asserted
the wrong emission count.

## 3. Findings

### CRITICAL — `APT::Status-Fd` floods the runner's 100-line output tail

`PrivilegedShell::run_command` retains only the last `OUTPUT_TAIL_LINES = 100` lines
(`src/runner.rs:16`) and returns that tail as the command output. `count_apt_upgraded`
(`src/backends/os_package_manager.rs:192`) parses the `"N upgraded, ..."` summary out of that
returned string — and that summary is printed *early*, right after the upgrade plan.

`APT::Status-Fd=1` emits a `pmstatus:`/`dlstatus:` line for every progress step of every
package, which pushes the summary out of the 100-line tail far sooner than plain apt output
would. The row would then report "0 packages updated" after a successful upgrade — a visible
regression in the feature fixed by commit c6495c3 ("show real updated items").

**Fix:** drop the `Status-Fd` option entirely and parse APT's ordinary output instead — the
`"N upgraded, M newly installed"` line already states the total, and `Unpacking <pkg>` /
`Setting up <pkg>` give two ticks per package. This removes the command change, the log-panel
filter and the `is_apt_status_line` helper: less code, no new failure mode, and no protocol
noise reaching either the tail or the log panel.

### RECOMMENDED — Nix `nixos-rebuild` fetch plans arrive per-command

`nix flake update` and `nixos-rebuild switch` run as one shell invocation, so both plan lines
land on the same tracker and are summed as intended. No change required; noted so the
sequential-command assumption is on record.

## 4. Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 100% | A |
| Best Practices | 95% | A |
| Functionality | 80% | B- |
| Code Quality | 95% | A |
| Security | 100% | A |
| Performance | 95% | A |
| Consistency | 100% | A |
| Build Success | N/A (host cannot build GTK; parser module verified standalone) | — |

**Overall Grade: B+ (88%)** — NEEDS_REFINEMENT on the APT finding.
