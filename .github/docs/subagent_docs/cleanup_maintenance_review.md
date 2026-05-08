# Cleanup / Maintenance Actions — Code Review

> Feature: `cleanup_maintenance`  
> Reviewer: Review Subagent (Phase 3)  
> Date: 2026-05-07  
> Verdict: **NEEDS_REFINEMENT**

---

## Build Validation Results

| Command | Exit Code | Result |
|---------|-----------|--------|
| `cargo fmt --check` | **1** | ❌ FAILED — 4 formatting diffs in `src/ui/window.rs` |
| `cargo clippy -- -D warnings` | 0 | ✅ PASSED |
| `cargo build` | 0 | ✅ PASSED |
| `cargo test` | 0 | ✅ PASSED (74 tests) |

---

## Formatting Failure Detail

`cargo fmt --check` reports four diff hunks, all in `src/ui/window.rs`. These are pure line-wrapping differences with no logic change — `rustfmt` wants the two `log_panel.append_line(...)` calls and the `async_channel::unbounded` binding reformatted to different line-split points:

```diff
-                log_panel
-                    .append_line("─── Maintenance started ───");
+                log_panel.append_line(
+                    "─── Maintenance started ───",
+                );

-                        let (event_tx, event_rx) =
-                            async_channel::unbounded::<OrchestratorEvent>();
+                        let (event_tx, event_rx) = async_channel::unbounded::<OrchestratorEvent>();

-                                    log_panel.append_line(
-                                        "Requesting administrator privileges…",
-                                    );
+                                    log_panel
+                                        .append_line("Requesting administrator privileges…");

-                                    log_panel
-                                        .append_line(&format!("Authentication failed: {e}"));
+                                    log_panel.append_line(&format!("Authentication failed: {e}"));
```

**Impact:** The preflight script (`scripts/preflight.sh`) runs `cargo fmt --check` and will exit non-zero, blocking CI. This is a CRITICAL blocker per the review workflow.

**Fix:** Run `cargo fmt` on the workspace (no logic changes required).

---

## Detailed Review Findings

### 1. Specification Compliance — 97%

All eight required changes from the spec are present:

| Spec Section | Requirement | Status |
|---|---|---|
| §4 `Backend` trait | `supports_cleanup()` default `false` | ✅ |
| §4 `Backend` trait | `run_cleanup()` no-op default | ✅ |
| §6.1 APT | `pkexec sh -c "DEBIAN_FRONTEND=noninteractive apt autoremove -y"`, `count_apt_autoremovals` | ✅ |
| §6.2 DNF | `pkexec dnf autoremove -y`, `count_dnf_autoremovals` | ✅ |
| §6.3 Pacman | Two-phase: unprivileged `pacman -Qtdq` → early return on empty → privileged `pkexec pacman -Rns --noconfirm <args…>` | ✅ |
| §6.4 Zypper | Two-phase: unprivileged `zypper packages --orphaned` → `is_safe_pkg_name` filter → privileged `pkexec sh -c "zypper remove -y <pkgs>"` | ✅ |
| §6.5 Nix | `nix-collect-garbage -d`, `count_nix_freed_paths` | ✅ |
| §6.6 Flatpak | `build_flatpak_cmd(["uninstall", "--unused", "-y"])` | ✅ |
| §6.7 Homebrew | `brew autoremove` then `brew cleanup`, `count_brew_cleaned` | ✅ |
| §5 `CleanupOrchestrator` | Parallel to `UpdateOrchestrator`, reuses `OrchestratorEvent`, correct auth logic | ✅ |
| §3.3 `set_status_cleaning` | Spinner, "Cleaning…", accent, retry hidden | ✅ |
| §3.3 `set_status_cleaned` | Count/clean message, success class, retry hidden | ✅ |
| §3.1 Menu | "Run Maintenance" above "About Up" | ✅ |
| §8.2 Action | `win.maintenance`, concurrency guard, event loop | ✅ |

**Minor deviation from spec:** The spec shows `#[weak] status_label` in the `maintenance_action.connect_activate` outer closure. The implementation uses `#[strong] status_label` in both the outer and inner closures. This is actually an improvement over the spec — using `#[strong]` prevents the label from being silently dropped while the async operation is pending. No correctness concern.

**Latent design note (inherited, not introduced by this feature):** On NixOS, `NixBackend::needs_root()` returns `true`. The `CleanupOrchestrator` therefore opens a privileged shell when NixBackend is present on NixOS, routing `nix-collect-garbage -d` through `pkexec`. The spec's §6.5 privilege table assumes `needs_root() = false` for Nix, which only holds on non-NixOS systems. On NixOS this causes system-level GC (root), which the spec's §6.5 note explicitly says to exclude. This is an inherited design gap in `NixBackend.needs_root()` — the cleanup feature faithfully implements the spec's instruction to reuse `needs_root()`, but the underlying method doesn't distinguish update-privilege from cleanup-privilege. This is a pre-existing architectural limitation, not a new defect introduced here; it is noted for future consideration (`needs_root_for_cleanup()` override).

---

### 2. Best Practices — 95%

- Async futures boxed correctly with `Pin<Box<...>>` and `+ Send + 'a` lifetimes. ✅
- No blocking I/O on the async executor (`tokio::process::Command` used for all subprocess calls). ✅
- Privileged commands always pass args as separate `&[&str]` elements, not interpolated into a single string — except for Zypper, where package names are joined into a shell string. Zypper correctly uses `is_safe_pkg_name` validation before this join. ✅
- `--noconfirm` (Pacman) and `-y` (APT, DNF, Zypper, Flatpak) prevent interactive prompts from blocking the persistent privileged shell. ✅
- Pacman's zero-orphan edge case is correctly handled (treats non-zero exit as "no orphans" when stdout is empty). ✅
- Zypper's zero-orphan check prevents calling `zypper remove` with no package arguments. ✅

**Gap:** No unit tests were added for the new cleanup-specific parser functions:
- `count_apt_autoremovals`
- `count_dnf_autoremovals`
- `parse_zypper_orphaned`
- `is_safe_pkg_name`
- `count_nix_freed_paths`
- `count_brew_cleaned`
- `run_cleanup` pipeline (MockExecutor-based) for each backend

The spec does not explicitly require these tests, but the project's existing pattern in `os_package_manager.rs` tests every parser and every `run_update` pipeline. The cleanup parsers lack this coverage.

---

### 3. Functionality — 98%

All cleanup flows produce correct `UpdateResult` variants:

| Backend | Zero case | Non-zero case | Error case |
|---------|-----------|---------------|------------|
| APT | `Success { 0 }` (no removals parsed) | `Success { N }` | `Error(e)` |
| DNF | `Success { 0 }` | `Success { N }` | `Error(e)` |
| Pacman | `Success { 0 }` (early return) | `Success { orphans.len() }` | `Error(spawn/exit)` |
| Zypper | `Success { 0 }` (early return) | `Success { orphans.len() }` | `Error(spawn/exit)` |
| Flatpak | `Success { 0 }` (no Uninstalling: lines) | `Success { N }` | `Error(e)` |
| Homebrew | `Success { 0 }` (no Removing lines) | `Success { N }` | `Error(e)` |
| Nix | `Success { 0 }` (no paths deleted line) | `Success { N }` | `Error(e)` |

The `SuccessWithSelfUpdate` branch in the `BackendFinished` window handler correctly falls through to `set_status_cleaned`, which is appropriate (a Flatpak self-update during cleanup is reported as cleaned). ✅

---

### 4. Code Quality — 88%

Strengths:
- `CleanupOrchestrator` is a near-verbatim parallel of `UpdateOrchestrator` — no duplication of logic, just a substitution of `run_cleanup` for `run_update`. ✅
- Helper functions (`count_apt_autoremovals`, `parse_zypper_orphaned`, `is_safe_pkg_name`, etc.) are small and focused. ✅
- All new functions are `pub(crate)` where needed for tests, private otherwise. ✅

Weaknesses:
- Formatting failure in `window.rs` (4 rustfmt diffs). ❌
- No unit tests for cleanup parsers — inconsistent with the module's existing test discipline. ⚠️

---

### 5. Security — 96%

| Risk | Mitigation | Status |
|------|------------|--------|
| Shell injection via orphan names (Zypper) | `is_safe_pkg_name` validates `[A-Za-z0-9._+-]+` before join | ✅ |
| Shell injection via orphan names (Pacman) | Args passed as discrete `&[&str]` elements, not into a shell string; runner validates `\n`/`\r`/`\0` | ✅ |
| Privilege escalation scope | Only backends where `needs_root() == true` get root; Flatpak, Homebrew run unprivileged | ✅ |
| External data interpolation (APT, DNF) | None — commands use fixed args only | ✅ |
| `DEBIAN_FRONTEND` env var | Set inside the sh command string, not via process env API; consistent with `run_update` pattern | ✅ |

No OWASP Top-10 issues found.

---

### 6. Performance — 95%

- Pacman and Zypper correctly perform a cheap unprivileged check before requesting elevated privilege. This avoids prompting the user for a password when there is nothing to clean. ✅
- Two-step Homebrew cleanup (`brew autoremove` then `brew cleanup`) is sequential but correct given the dependency between the two operations. ✅
- No unnecessary allocations in parsers. ✅

---

### 7. Consistency — 95%

- `CleanupOrchestrator` follows the exact same structural pattern as `UpdateOrchestrator` (auth → shell → log forwarding → backend loop → close). ✅
- `set_status_cleaning` and `set_status_cleaned` follow the exact same pattern as `set_status_running` and `set_status_success` respectively. ✅
- The `maintenance_action` handler mirrors the `update_button.connect_clicked` handler structure. ✅
- Log format `[{kind}] {line}` is identical to the update flow. ✅
- The "Run Maintenance" item appears first in the menu, above "About Up" per spec §3.1. ✅
- `update_in_progress` flag is set/cleared symmetrically in all code paths (including the `AuthFailed` early-return path). ✅

---

### 8. Build Success — 75%

| Check | Result |
|-------|--------|
| `cargo fmt --check` | ❌ FAILED |
| `cargo clippy -- -D warnings` | ✅ PASSED |
| `cargo build` | ✅ PASSED |
| `cargo test` (74 tests) | ✅ PASSED |

---

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 97% | A |
| Best Practices | 95% | A |
| Functionality | 98% | A+ |
| Code Quality | 88% | B+ |
| Security | 96% | A |
| Performance | 95% | A |
| Consistency | 95% | A |
| Build Success | 75% | C |

**Overall Grade: B+ (92%)**

---

## Issues Summary

### CRITICAL (blocks CI/preflight)

1. **`cargo fmt --check` fails** — 4 formatting diffs in `src/ui/window.rs`. Logic is correct; only line-wrapping style diverges from `rustfmt` defaults.  
   **Fix:** Run `cargo fmt` (or `cargo fmt -- src/ui/window.rs`) and commit the result. No functional change required.

### RECOMMENDED (not blocking but should be addressed)

2. **Missing unit tests for cleanup parser functions** — `count_apt_autoremovals`, `count_dnf_autoremovals`, `parse_zypper_orphaned`, `is_safe_pkg_name`, `count_nix_freed_paths`, `count_brew_cleaned`, and `run_cleanup` pipeline tests (MockExecutor-based) for each backend. The module's existing test discipline covers all update parsers and pipelines; cleanup parsers should be consistent.

### INFORMATIONAL (future consideration, not actionable now)

3. **NixOS cleanup privilege** — On NixOS, `NixBackend::needs_root()` returns `true`, causing `nix-collect-garbage -d` to run through the privileged shell (root). The spec intended user-level GC only. A future `needs_root_for_cleanup()` override on `NixBackend` returning `false` would address this without breaking the update flow.

---

## Verdict

**NEEDS_REFINEMENT**

The implementation is functionally correct and well-structured. The single blocking issue is a `cargo fmt` formatting failure in `src/ui/window.rs` that will cause the preflight script and CI to fail. The fix is trivial (run `cargo fmt`) and requires no logic changes.
