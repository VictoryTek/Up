# Review: Nix Check Fix — v2.0.2

**Feature ID:** `nix_check_fix`  
**Reviewer pass:** Phase 3 — Review & Quality Assurance  
**Date:** 2026-05-14  
**Verdict:** **NEEDS_REFINEMENT**

---

## Files Reviewed

| File | Status |
|------|--------|
| `src/backends/nix.rs` | Reviewed |
| `src/ui/update_row.rs` | Reviewed |
| `src/ui/window.rs` | Reviewed |
| `Cargo.toml` | Reviewed |
| `meson.build` | Reviewed (no changes needed — derives version from Cargo.toml) |
| `data/io.github.up.metainfo.xml` | Reviewed |
| `releases/2.0.2.md` | Reviewed |

---

## Build Results

### `cargo build`
**PASS** — Compiled without errors.
```
warning: Git tree '/home/nimda/Projects/Up' is dirty
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s
```

### `cargo clippy -- -D warnings`
**PASS** — Zero warnings, zero errors.
```
Finished `dev` profile [unoptimized + debuginfo] target(s) in 23.76s
```

### `cargo fmt --check`
**FAIL** — Formatting diff detected in `src/ui/window.rs` line 1105.
```
Diff in /home/nimda/Projects/Up/src/ui/window.rs:1105:
                                                 "Could not check all sources.",
                                             ));
                                         } else {
-                                            status_label_checks.set_label(&gettext(
-                                                "Everything is up to date.",
-                                            ));
+                                            status_label_checks
+                                                .set_label(&gettext("Everything is up to date."));
                                         }
```
The `"Everything is up to date."` call was written in multi-line form; rustfmt prefers the chained method form at this indentation depth.

### `cargo test`
**PASS** — 99/99 tests pass.
```
running 99 tests
... (all pass)
test result: ok. 99 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

---

## Findings by Criterion

### 1. Specification Compliance

**Score: 70% — C**

#### Bug 2 (Nix stale cache) — CORRECT ✓

Both `nixos_flake_dry_run_check()` and `nixos_flake_tempdir_check()` in `src/backends/nix.rs` now include `--option eval-cache false --option tarball-ttl 0` as specified. The `run_update` path correctly omits these flags (as required). Implementation matches the spec exactly.

#### Bug 1 (false "Everything is up to date") — CRITICAL DEVIATION ✗

The spec (section 4.1 and 5.1.1) requires:

```rust
// Spec requires:
check_errored: Rc<Cell<bool>>,
```

The implementation uses:

```rust
// What was implemented:
check_errored: Cell<bool>,
```

**This deviation breaks the fix entirely.** `UpdateRow` is `#[derive(Clone)]`. The window clones a row from the `rows` vec before calling status methods on it:

```rust
// window.rs — clones the row, then calls set_status_unknown on the clone
let row = {
    let borrowed = rows.borrow();
    borrowed.iter().find(|(k, _)| *k == kind_async).map(|(_, r)| r.clone())
};
// ...
row.set_status_unknown(&msg);  // ← sets check_errored on the CLONE
```

Later, the error check reads from rows in the `rows` vec (not the clone):

```rust
let any_error = {
    let borrowed = rows.borrow();
    borrowed.iter().filter(...).any(|(_, r)| r.check_errored())
};
```

With `Rc<Cell<bool>>`, clone and original share the same underlying `Cell`; mutation through the clone is visible to the original. With plain `Cell<bool>`, `#[derive(Clone)]` creates an independent copy; mutation through the clone is **invisible** to the original row in `rows`. As a result, `any_error` is always `false`, "Could not check all sources." is never displayed, and the headline continues to show the false-positive "Everything is up to date." — identical to the pre-fix behaviour.

All other shared-state fields in `UpdateRow` (`skip_flag`, `last_available`, `estimated_bytes`, `updating_parent`) correctly use `Rc<Cell<...>>`. The `check_errored` field is the sole outlier and must be changed to `Rc<Cell<bool>>`.

#### Minor: field initialisation order differs from spec

The spec shows `check_errored.set(false)` after `estimated_bytes` in `set_status_checking()`. The implementation places it before `last_available`. Functionally identical — not a defect.

---

### 2. Best Practices

**Score: 80% — B**

All other patterns in the file follow the established `Rc<Cell<...>>` convention for shared mutable state across clones. The `check_errored: Cell<bool>` deviation is inconsistent with this convention and would confuse future contributors about why this field behaves differently from `last_available`, `skip_flag`, etc.

The accessor `pub fn check_errored(&self) -> bool` and the docstrings are well-written. The `set_status_unknown()` docstring update (explaining that it sets `check_errored`) is accurate and helpful.

The Nix backend changes in `nix.rs` follow idiomatic `.args([...])` chaining consistently with the rest of the file.

---

### 3. Functionality

**Score: 50% — D**

- **Bug 2 fix:** Fully functional. `--option eval-cache false --option tarball-ttl 0` will bypass Nix's evaluation and tarball caches for both the dry-run and tempdir check paths. ✓
- **Bug 1 fix:** Non-functional due to `Cell<bool>` vs `Rc<Cell<bool>>`. The "Could not check all sources." message will never be shown. ✗

---

### 4. Code Quality

**Score: 75% — C**

`cargo fmt --check` fails with one diff in `window.rs`. The `"Everything is up to date."` label call was formatted in a multi-line style that rustfmt normalises to the chained method form. No other formatting issues.

The diff is minimal and surgical — no unnecessary changes beyond what is required for the two fixes and the version bump. Indentation and line length are consistent with the rest of each file. No dead code or commented-out fragments introduced.

---

### 5. Security

**Score: 100% — A**

No new external inputs are processed. The Nix option flags (`--option eval-cache false`, `--option tarball-ttl 0`) are static string literals with no interpolation. No new privilege escalation paths. The `check_errored` flag is local UI state with no security implications.

---

### 6. Performance

**Score: 100% — A**

No additional blocking operations. The cache-bypass flags cause Nix to re-fetch from the network during checks, which is the intended behaviour and does not affect the update path. The `check_errored` flag involves only `Cell::get()`/`Cell::set()` — negligible overhead.

---

### 7. Consistency

**Score: 75% — B**

Inconsistency with the `Rc<Cell<...>>` pattern used by all other shared state fields in `UpdateRow`. All fields that must propagate mutations through clones (`skip_flag`, `last_available`, `estimated_bytes`, `updating_parent`) use `Rc<Cell<...>>`. `check_errored: Cell<bool>` breaks this convention.

All other aspects (naming, comment style, module structure, file placement) are consistent with the existing codebase.

---

### 8. Build Success

**Score: 75% — C**

| Check | Result |
|-------|--------|
| `cargo build` | ✅ PASS |
| `cargo clippy -- -D warnings` | ✅ PASS |
| `cargo fmt --check` | ❌ FAIL |
| `cargo test` | ✅ PASS (99/99) |

`cargo fmt --check` is a hard requirement per the review criteria. Its failure makes this category a non-pass.

---

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 70% | C |
| Best Practices | 80% | B |
| Functionality | 50% | D |
| Code Quality | 75% | C |
| Security | 100% | A |
| Performance | 100% | A |
| Consistency | 75% | B |
| Build Success | 75% | C |

**Overall Grade: C (78%)**

---

## Required Fixes (Phase 4)

### CRITICAL — Must fix before approval

**Fix 1: `check_errored` type in `src/ui/update_row.rs`**

Change the struct field from:
```rust
check_errored: Cell<bool>,
```
to:
```rust
check_errored: Rc<Cell<bool>>,
```

Update the initialisation in `new()` from:
```rust
check_errored: Cell::new(false),
```
to:
```rust
check_errored: Rc::new(Cell::new(false)),
```

The `set_status_checking()`, `set_status_unknown()`, and `check_errored()` accessor call sites (`self.check_errored.set(...)` / `self.check_errored.get()`) require no changes — `Rc<Cell<bool>>` exposes `Deref<Target=Cell<bool>>`, so the call syntax is identical.

**Fix 2: Formatting diff in `src/ui/window.rs`**

Run `cargo fmt` on the workspace to apply the rustfmt-normalised form of the "Everything is up to date." label call, then confirm `cargo fmt --check` passes.

---

## Verdict

**NEEDS_REFINEMENT**

Two issues block approval:

1. **Critical functional bug:** `check_errored: Cell<bool>` does not propagate through `UpdateRow` clones; must be `Rc<Cell<bool>>` as specified. Bug 1 is not fixed.
2. **CI gate failure:** `cargo fmt --check` fails with a formatting diff in `window.rs`.

Both fixes are small and low-risk. Once applied, all four build gates should pass and both bugs should be correctly resolved.
