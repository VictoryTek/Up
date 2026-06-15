# Review: Fix NixOS (VexOS) False Positive and Flatpak False Negative

**Feature:** `nix_flatpak_check_bugs`
**Date:** 2026-06-14
**Spec:** `.github/docs/subagent_docs/nix_flatpak_check_bugs_spec.md`

---

## 1. Change Verification

### Fix 1 — VexOS: delegate to `nixos_flake_changed_inputs()` ✅ APPLIED

`src/backends/nix.rs` lines 619–628:
```rust
if is_vexos() {
    // VexOS uses vexos-update for the actual update, but we can still
    // detect whether upstream flake inputs have changed before claiming an
    // update is available — the same nixos_flake_changed_inputs() check used
    // for standard NixOS flake systems works identically here.
    match nixos_flake_changed_inputs().await {
        Ok(inputs) if inputs.is_empty() => Ok(vec![]),
        Ok(_) => Ok(vec!["NixOS system".to_string()]),
        Err(e) => Err(e),
    }
```

Previous behaviour: always returned `["NixOS system"]` — always 1 update.
New behaviour: returns empty when no flake inputs changed, `["NixOS system"]` when they did.
`run_update()` for VexOS unchanged. `supports_item_selection()` unchanged (already excludes VexOS).

### Fix 2 — Flatpak: `--columns=name` instead of `--columns=application` ✅ APPLIED

`src/backends/flatpak.rs` line 278:
```rust
let (cmd, args) = build_flatpak_cmd(&["remote-ls", "--updates", scope, "--columns=name"]);
```

The `name` column returns the ref name for both apps AND runtimes. The previous
`application` column was empty for runtimes (e.g., `org.gnome.Platform`), causing
them to be silently filtered out as blank lines.

### Fix 3 — Flatpak: header filter updated ✅ APPLIED

`src/backends/flatpak.rs` lines 311–318:
```rust
if !t.is_empty() && !t.eq_ignore_ascii_case("name") && !t.eq_ignore_ascii_case("application") {
```

Both "Name" (new header) and "Application" (old header) are filtered.

### Fix 4 — Flatpak: error handling ✅ APPLIED

`src/backends/flatpak.rs` lines 139–156: the `(Ok([]), Err(_)) => Ok([])` arm is
replaced with: if the successful scope is empty and the other errored, propagate
the error to avoid a silent false "up to date".

### Fix 5 — `UpdateRow`: `check_errored` flag ✅ APPLIED

- `check_errored: Rc<Cell<bool>>` field added to struct
- Initialised to `false` in constructor
- `set_status_checking()` resets to `false`
- `set_status_unknown()` sets to `true`
- `has_check_error() -> bool` accessor added

### Fix 6 — `window.rs`: accurate headline ✅ APPLIED

After the existing `non_skipped_total` computation, an `any_check_error` block
iterates non-skipped rows checking `has_check_error()`. If true, headline shows
"Could not check all sources." instead of "Everything is up to date."

---

## 2. Build Results

| Check | Result |
|-------|--------|
| `cargo fmt --check` | ✅ PASS — no formatting issues |
| `cargo clippy -- -D warnings` | ✅ PASS — zero warnings |
| `cargo test` | ✅ PASS — 99 tests pass, 0 failures |

Test details (flatpak + nix):
- `test_parse_flatpak_app_line_header_skipped` — now tests both "Application"/"application" AND "Name"/"name" ✅
- `test_parse_flatpak_updates_happy_path` — updated to use "Name" header and includes runtime IDs ✅
- `test_parse_flatpak_updates_only_header` — updated to use "Name" header ✅
- All existing Nix tests pass unchanged ✅

---

## 3. Specification Compliance

| Requirement | Status |
|------------|--------|
| VexOS uses `nixos_flake_changed_inputs()` | ✅ |
| Returns empty when no flake inputs changed | ✅ |
| Returns `["NixOS system"]` when inputs changed | ✅ |
| `run_update()` and `supports_item_selection()` unchanged | ✅ |
| `--columns=name` in `flatpak_remote_ls_updates` | ✅ |
| "Name" + "Application" headers both filtered | ✅ |
| Error handling: empty+error → propagate error | ✅ |
| `check_errored` field in `UpdateRow` | ✅ |
| `set_status_checking()` resets flag | ✅ |
| `set_status_unknown()` sets flag | ✅ |
| `has_check_error()` accessor | ✅ |
| Window headline uses `any_check_error` | ✅ |

---

## 4. Security Review

- No new external command execution paths
- `nixos_flake_changed_inputs()` already had security review (cache-bypass flags)
- Flatpak command argument list unchanged except the column name string — not user-controlled
- No new network access
- `check_errored` flag: no security impact (local GTK thread state)

---

## 5. Score Table

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

---

## 6. Verdict

**PASS**

All spec requirements implemented correctly. Build, lint, format, and test suite pass.
No issues found. Ready for Phase 6 preflight.
