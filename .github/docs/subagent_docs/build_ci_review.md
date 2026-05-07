# Build / CI Review — `build_ci`

**Reviewer:** Quality Assurance Subagent  
**Spec:** `.github/docs/subagent_docs/build_ci_spec.md`  
**Date:** 2026-05-07  
**Verdict:** ⚠️ NEEDS_REFINEMENT

---

## Build Validation Results

| Check | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --check` | ✅ PASS (exit 0) |
| Lint | `cargo clippy -- -D warnings` (nix develop) | ✅ PASS (exit 0) |
| Build | `cargo build` (nix develop) | ✅ PASS (exit 0) |
| Tests | `cargo test` (nix develop) | ✅ PASS (74 passed, 0 failed) |
| Preflight syntax | `bash -n scripts/preflight.sh` | ✅ PASS (exit 0) |
| Preflight runtime | `nix develop --command bash scripts/preflight.sh` | ❌ FAIL (exit 101) |

> **Note:** This is a NixOS development environment. GTK4/libadwaita system libraries are only available inside `nix develop`. All Rust compilation steps were run inside the dev shell; bare `cargo clippy` outside the shell fails as expected.  
> The preflight runtime failure is the direct cause of NEEDS_REFINEMENT — see Critical Finding #1.

---

## Findings by File

---

### 1. `.github/workflows/release.yml` — PASS (minor notes)

| Check | Status | Notes |
|---|---|---|
| Triggers on `v*.*.*` | ✅ | `on: push: tags: - 'v*.*.*'` |
| `ci-checks` job mirrors `native-build` | ✅ | Same runner (ubuntu-24.04), same toolchain, same cache |
| `release` job `needs: [ci-checks]` | ✅ | Dependency correctly declared |
| Nix install via `cachix/install-nix-action@v31` | ✅ | Correct version |
| `nix build` runs | ✅ | No extra flags needed |
| Binary from `./result/bin/up` | ✅ | Correct path |
| `softprops/action-gh-release@v2` | ✅ | Correct action |
| `generate_release_notes: true` | ✅ | Auto release notes |
| `permissions: contents: write` | ⚠️ | Set at **top-level** (affects all jobs). Spec's authoritative YAML places it only on the `release` job. The `ci-checks` job inherits `contents: write` unnecessarily. Not a blocker, but violates principle of least privilege. |

**Action required (LOW):** Move `permissions: contents: write` from the top-level to the `release` job only. Add explicit `permissions: read-all` (or no `permissions` block) on `ci-checks`.

---

### 2. `meson.build` — PASS

| Check | Status | Notes |
|---|---|---|
| `build_always_stale: true` removed | ✅ | Not present |
| `depend_files: files('Cargo.toml', 'Cargo.lock')` | ✅ | Present |
| `--target-dir` points into build dir | ✅ | `meson.build_root() / 'cargo-target'` |
| `cp` source is build dir, not srcdir | ✅ | `meson.build_root() / 'cargo-target' / rust_target / 'up'` |

**Observation (COSMETIC):** `builddir = meson.current_build_dir()` is already declared and used for other paths in the file. The `custom_target` uses `meson.build_root()` directly instead of the established `builddir` variable. Functionally equivalent in a top-level `meson.build`, but inconsistent with the rest of the file.

---

### 3. `scripts/preflight.sh` — NEEDS_REFINEMENT

#### Step 7 (cargo audit): ✅ PASS

```bash
if command -v cargo-audit &>/dev/null || cargo audit --version &>/dev/null 2>&1; then
    cargo audit
else
    echo "Notice: cargo-audit not installed, skipping audit."
fi
```

Graceful skip is present. The extra `command -v cargo-audit` check is more reliable than the spec's single `cargo audit --version` test. Minor: `2>&1` after `&>/dev/null` is redundant (bash's `&>` already redirects both stdout and stderr) but harmless.

#### Step 8 (nix flake check): ❌ CRITICAL — Fatal on failure

**Actual implementation:**
```bash
echo "--- Step 8: Nix flake check ---"
if command -v nix &>/dev/null; then
    nix flake check          # ← no error handler; script aborts on failure due to set -euo pipefail
else
    echo "Notice: nix not found, skipping flake check."
fi
```

**Spec requires:**
```bash
echo "--- Step 8: Nix flake check ---"
if [[ -f flake.nix ]] && command -v nix &>/dev/null; then
    nix flake check 2>/dev/null && echo "Nix flake check passed." || echo "Notice: nix flake check could not complete (may require full Nix evaluation); skipping."
else
    echo "Notice: Nix not available or no flake.nix found, skipping nix flake check."
fi
```

**Impact:** The spec explicitly requires `|| echo "Notice: ..."` to make the step non-fatal. Without it, `nix flake check` failures (including failures caused by a dirty git tree — the normal state during local development) abort the entire preflight with exit 101. This was confirmed by the terminal context:

```
nix develop --command bash scripts/preflight.sh 2>&1
Exit Code: 101
```

This means `scripts/preflight.sh` **cannot successfully complete** in a typical development environment with a dirty working tree.

**Action required (CRITICAL):** Add the `|| echo "Notice: ..."` safety handler to step 8. Also add the `[[ -f flake.nix ]]` guard as specified.

---

### 4. `rust-toolchain.toml` — PASS

```toml
[toolchain]
channel = "stable"
```

Matches spec exactly. No version pin — intentionally consistent with the spec's rationale.

---

### 5. `.editorconfig` — MINOR DEVIATIONS

| Check | Status | Notes |
|---|---|---|
| `root = true` | ✅ | Present |
| `charset = utf-8` | ✅ | Present in `[*]` |
| `end_of_line = lf` | ✅ | Present |
| `insert_final_newline = true` | ✅ | Present |
| `trim_trailing_whitespace = true` | ✅ | Present |
| 4-space indent for `*.{rs,toml}` | ✅ | Present |
| 2-space indent for `*.{yml,yaml,json}` | ✅ | Present (grouped as `*.{yml,yaml,json}`) |
| 2-space indent for `*.xml` | ✅ | Present (separate section) |
| `[*.md]` with `trim_trailing_whitespace = false` | ❌ | **Missing.** Spec includes this section because trailing spaces are semantic in Markdown. Without it, the `[*]` global `trim_trailing_whitespace = true` incorrectly strips meaningful line breaks from `.md` files. |
| `indent_size = 4` for `[Makefile]` | ⚠️ | `indent_style = tab` is present but `indent_size = 4` is absent. Minor. |

**Action required (LOW):** Add `[*.md]` section with `trim_trailing_whitespace = false`. Optionally add `indent_size = 4` to `[Makefile]`.

---

### 6. `cargo-sources.json` — PASS

Confirmed absent from repo root. Directory listing shows no `cargo-sources.json`. ✅

---

### 7. `ci.yml` — NEEDS_REFINEMENT (Item 7.8 not implemented)

The spec (Item 7.8, Section 2.3.2) requires adding a `cargo-audit` job to `ci.yml`:

```yaml
  cargo-audit:
    name: Security Audit
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v6
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with:
          cache-on-failure: true
      - run: cargo install cargo-audit --locked
      - run: cargo audit
```

It also requires the `validation` job to declare:
```yaml
needs: [native-build, cargo-audit]
```

**Current state of `ci.yml`:**
- `cargo-audit` job: **MISSING**
- `validation` job `needs`: only `[native-build]` — **not updated**

This is a HIGH severity gap: CI has no automated security advisory scanning despite it being a named spec requirement.

**Action required (HIGH):** Add `cargo-audit` job to `ci.yml`; update `validation` job `needs` and summary output.

---

## Issue Summary

| # | Severity | File | Issue |
|---|---|---|---|
| 1 | **CRITICAL** | `scripts/preflight.sh` | Step 8 `nix flake check` is fatal — missing `\|\| echo "Notice..."` handler. Script exits 101 on dirty tree. |
| 2 | **HIGH** | `.github/workflows/ci.yml` | `cargo-audit` job not added; `validation` needs not updated. Spec Item 7.8 partially unimplemented. |
| 3 | LOW | `.editorconfig` | Missing `[*.md]` section with `trim_trailing_whitespace = false`. |
| 4 | LOW | `.github/workflows/release.yml` | `permissions: contents: write` at top-level instead of only on `release` job (minor overprivilege). |
| 5 | COSMETIC | `.editorconfig` | Missing `indent_size = 4` in `[Makefile]` section. |
| 6 | COSMETIC | `meson.build` | `meson.build_root()` used instead of existing `builddir` variable (inconsistent style, functionally correct). |

---

## Score Table

| Category | Score | Grade |
|---|---|---|
| Specification Compliance | 68% | D+ |
| Best Practices | 80% | B- |
| Functionality | 65% | D |
| Code Quality | 85% | B |
| Security | 70% | C |
| Performance | 95% | A |
| Consistency | 83% | B |
| Build Success | 88% | B+ |

**Overall Grade: C+ (79%)**

> Score penalized primarily by: missing `ci.yml` cargo-audit job (spec item 7.8 partially unimplemented), and the broken preflight step 8 that makes local `scripts/preflight.sh` fail at runtime.

---

## Verdict

**NEEDS_REFINEMENT**

### Critical (must fix before approval)
1. `scripts/preflight.sh` — Add `|| echo "Notice: ..."` to step 8 so `nix flake check` failures are non-fatal.

### High (must fix before approval)
2. `.github/workflows/ci.yml` — Add `cargo-audit` job; update `validation` job `needs` and summary.

### Low (should fix)
3. `.editorconfig` — Add `[*.md]` with `trim_trailing_whitespace = false`.
4. `.github/workflows/release.yml` — Scope `permissions: contents: write` to the `release` job only.

### Cosmetic (optional)
5. `.editorconfig` — Add `indent_size = 4` under `[Makefile]`.
6. `meson.build` — Use `builddir` variable instead of `meson.build_root()` for consistency.
