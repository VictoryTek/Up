# Final Review: True Package-Level Progress Bar

**Spec:** `.github/docs/subagent_docs/true_progress_bar_spec.md`
**Prior review:** `.github/docs/subagent_docs/true_progress_bar_review.md`
**Refinement cycle:** 1 of 2
**Result:** APPROVED (with one deferred gate — see §4)

---

## 1. CRITICAL Issue Resolution

**Finding:** `-o APT::Status-Fd=1` would flood the runner's 100-line output tail
(`src/runner.rs:16`), evicting the `"N upgraded, ..."` summary that `count_apt_upgraded`
depends on, and regressing the per-row updated count.

**Resolution:** the Status-Fd approach was removed entirely.
`src/backends/os_package_manager.rs` is now byte-identical to `HEAD` (`git diff` reports no
change), as are the log-panel filter and the `is_apt_status_line` helper in `src/ui/window.rs`.
APT progress is instead derived from apt's ordinary output, which the tracker already sees:

- Total: `2 upgraded, 1 newly installed, 0 to remove and 0 not upgraded.` → 3 packages
- Ticks: `Unpacking <pkg> ...` and `Setting up <pkg> ...`, two per package, de-duplicated

No command changes, no protocol noise in the tail or the log panel, and less code than the
reviewed version.

## 2. Final Change Set

| File | Change |
|---|---|
| `src/progress.rs` (new, 568 lines incl. tests) | `ProgressTracker` + parsers for Nix, Flatpak, APT, DNF, Pacman/Zypper, Homebrew, fwupd, plugins; 17 unit tests |
| `src/main.rs` | `mod progress;` |
| `src/orchestrator.rs` | `OrchestratorEvent::BackendProgress(BackendKind, f64)`; tracker driven from the existing log-forwarding task |
| `src/ui/window.rs` | `BackendProgress` handler (monotonic, segment-relative); `BackendStarted` sets the segment floor instead of a fake half-step; no-op arm in the cache-bypass loop |

`git diff --stat`: `src/main.rs` +1, `src/orchestrator.rs` +22/-1, `src/ui/window.rs` +21/-3.

## 3. Verification Performed

```
rustfmt --edition 2021 --check src/progress.rs src/orchestrator.rs src/ui/window.rs src/main.rs
    → FMT CLEAN
cargo test   (src/progress.rs in a GTK-free harness crate)
    → test result: ok. 17 passed; 0 failed
cargo clippy --all-targets -- -D warnings   (same harness)
    → Finished, no warnings
```

Behavioural review of the wiring (cannot be compiled on this host, see §4):

- `BackendKind` derives `PartialEq`, required by the tracker-reset comparison — confirmed at `src/backends/mod.rs:73`.
- Progress events are sent from the same task, on the same channel, after the log line they were derived from, so ordering with `BackendLog` is preserved.
- The retry event loop (`src/ui/window.rs:~940`) already has a `_ => {}` arm; the cache-bypass loop is exhaustive and received an explicit no-op arm.
- `BackendStarted` now sets `finished / total`, which equals the value `BackendFinished` set for the previous backend — the bar cannot step backwards at a segment boundary.

## 4. Deferred Gate — Preflight

`scripts/preflight.sh` **has not been run**. This host is Windows and `gtk4-sys` cannot
configure (`pkg-config` not found), so `cargo build`, `cargo test` and `cargo clippy` for the
`up` crate itself are impossible here — verified by running `cargo check`, not assumed. The
parser module was validated standalone; the three wiring edits are compile-gated only.

**The user must run `scripts/preflight.sh` on the Linux machine before committing.**

## 5. Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 100% | A |
| Best Practices | 100% | A |
| Functionality | 95% | A |
| Code Quality | 100% | A |
| Security | 100% | A |
| Performance | 100% | A |
| Consistency | 100% | A |
| Build Success | Deferred — host cannot build GTK4 | — |

**Overall Grade: A (99% of what is verifiable on this host)**
