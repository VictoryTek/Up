# Review: Nix Upgrade Fix & Upgrade Tab Visibility

**Feature:** `nix_upgrade_fix`  
**Date:** 2026-04-24  
**Reviewer:** Review Subagent  
**Spec:** `.github/docs/subagent_docs/nix_upgrade_fix_spec.md`  

---

## Files Reviewed

- `src/backends/nix.rs`
- `src/backends/mod.rs`
- `src/upgrade.rs`
- `src/backends/determinate_nix.rs` (confirmed deleted)

---

## Build Results

| Check | Exit Code | Result |
|-------|-----------|--------|
| `cargo fmt --check` | **1** | ❌ **FAILED** — 3 formatting diffs |
| `cargo clippy -- -D warnings` | 0 | ✅ PASSED |
| `cargo build` | 0 | ✅ PASSED |
| `cargo test` | 0 | ✅ PASSED — 18/18 |

### Formatting Diffs (`cargo fmt --check`)

**`src/backends/nix.rs:271`** — `is_determinate_nix()` body  
rustfmt requires single-line `&&` expression:
```rust
// Current (rejected by rustfmt):
fn is_determinate_nix() -> bool {
    std::path::Path::new("/nix/receipt.json").exists()
        && which::which("determinate-nixd").is_ok()
}

// Required:
fn is_determinate_nix() -> bool {
    std::path::Path::new("/nix/receipt.json").exists() && which::which("determinate-nixd").is_ok()
}
```

**`src/backends/nix.rs:475`** — `Ok(if ...)` expression  
rustfmt requires expanded if/else block:
```rust
// Current (rejected by rustfmt):
Ok(if upgrade_available_in_output(&combined) { 1 } else { 0 })

// Required:
Ok(if upgrade_available_in_output(&combined) {
    1
} else {
    0
})
```

**`src/upgrade.rs:95`** — `id_like` method chain  
rustfmt requires single-line chain:
```rust
// Current (rejected by rustfmt):
let id_like = fields
    .get("ID_LIKE")
    .cloned()
    .unwrap_or_default();

// Required:
let id_like = fields.get("ID_LIKE").cloned().unwrap_or_default();
```

**Fix:** Run `cargo fmt` in the project root. This is the only required change.

---

## Specification Compliance

### Issue 1: Determinate Nix merged into NixBackend ✅
- `src/backends/determinate_nix.rs` is deleted (confirmed — file not found).
- No `DeterminateNixBackend` or `determinate_nix` module references exist in any source file.
- `BackendKind` enum in `mod.rs` no longer contains a `DeterminateNix` variant.
- `detect_backends()` only registers `NixBackend`.

### Issue 2: Nix profile upgrade command fixed ✅
- `run_update()` non-NixOS branch now correctly inverts the manifest fallback:
  - Unreadable manifest → `use_legacy_nix_env = false` → `nix profile upgrade '.*'` (correct default).
  - Confirmed readable v1 manifest → `use_legacy_nix_env = true` → `nix-env -u` (legacy compat).
- `count_available()` and `list_available()` apply the same manifest check logic.
- `nix profile upgrade` is invoked with `--extra-experimental-features nix-command` per spec.

### Issue 3: Determinate Nix runs without pkexec ✅
- `run_update()` Determinate Nix branch: `runner.run("determinate-nixd", &["upgrade"])` — no pkexec.
- `needs_root()` returns `is_nixos()` which is `false` for Determinate Nix systems (correct).
- `count_available()` and `list_available()` use direct `tokio::process::Command::new("determinate-nixd")`.

### Issue 4: Upgrade tab visibility fixed ✅
- `upgrade_supported` in `detect_distro()` now covers:
  - `"ubuntu"` → `true` (unconditional, no `do-release-upgrade` gating)
  - `"linuxmint"`, `"pop"`, `"elementary"`, `"zorin"` → `true`
  - `"fedora"`, `"opensuse-leap"`, `"debian"`, `"nixos"` → `true`
  - `"rhel"`, `"centos"` → `true` (extra addition beyond spec scope — benign)
  - `ID_LIKE=ubuntu` derivatives → `true`
  - `ID_LIKE=debian` derivatives → `true`

### Helper functions migrated ✅
- `is_determinate_nix()` — detection matches spec (two-marker check).
- `upgrade_available_in_output()` — case-insensitive "an upgrade is available" check.
- `count_determinate_upgraded()` — handles nothing-to-upgrade/already-up-to-date/success cases.
- `description()` includes Determinate Nix branch per spec.

### Tests ✅
- 4 new tests covering `upgrade_available_in_output` and `count_determinate_upgraded`.
- All 18 tests pass.

---

## Additional Observations

### Out-of-Spec Addition
`"rhel" | "centos" => true` was added to `upgrade_supported` but is not mentioned in the spec. The `execute_upgrade()` function does not handle these IDs and will return a "not supported" error at runtime, which is the correct UX behaviour per the spec's rationale. This is a minor benign addition.

### Manifest Check Code Duplication
The `use_legacy_nix_env` block appears three times (once each in `run_update()`, `count_available()`, `list_available()`). This is consistent with the existing project style (no premature abstraction), acceptable as-is, and not a defect.

### Security
- `validate_flake_attr()` guards all flake attribute interpolation — no shell injection risk.
- `determinate-nixd` correctly runs without elevated privileges.
- No new attack surface introduced.

---

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 97% | A |
| Best Practices | 90% | A- |
| Functionality | 100% | A+ |
| Code Quality | 85% | B+ |
| Security | 100% | A+ |
| Performance | 95% | A |
| Consistency | 95% | A |
| Build Success | 75% | C |

**Overall Grade: B+ (92%)**  
*Build score penalised for `cargo fmt --check` failure.*

---

## Summary of Findings

All four spec issues are correctly implemented:
1. `DeterminateNixBackend` and its module are fully removed; Determinate Nix logic is merged into `NixBackend`.
2. The manifest fallback is inverted — modern `nix profile upgrade '.*'` is now the default.
3. `determinate-nixd upgrade` runs without `pkexec`.
4. The upgrade tab shows for Ubuntu, Debian, Mint, Pop, and their `ID_LIKE` derivatives without gating on `do-release-upgrade`.

The sole blocking issue is **3 formatting diffs** that cause `cargo fmt --check` to fail. The fix is to run `cargo fmt` — no logic changes are needed.

---

## Verdict

**NEEDS_REFINEMENT**

**Critical issue:** `cargo fmt --check` fails with 3 formatting diffs in `src/backends/nix.rs` and `src/upgrade.rs`.  
**Required fix:** Run `cargo fmt` in the project root.  
No logic defects, no security issues, no failing tests. The single remediation required is formatting-only.
