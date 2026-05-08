# Changelog Viewer — Review & QA Report

**Feature:** Update changelog viewer  
**Reviewer:** QA Subagent  
**Date:** 2026-05-08  
**Status:** NEEDS_REFINEMENT

---

## Build Validation

### `cargo fmt --check`

```
Exit code: 0 — no formatting issues
```

`cargo build` / `cargo clippy` cannot be run on Windows (GTK4 not installed — expected for this environment). `cargo fmt --check` passes cleanly, which is the only locally executable build validation.

---

## Files Reviewed

| File | Status |
|------|--------|
| `src/changelog.rs` (new) | Reviewed |
| `src/main.rs` | Reviewed |
| `src/ui/update_row.rs` | Reviewed |
| `src/backends/mod.rs` | Reviewed (BackendKind reference) |
| `src/ui/window.rs` | Reviewed (UpdateRow::new call sites) |
| `.github/docs/subagent_docs/changelog_viewer_spec.md` | Reviewed (source of truth) |

---

## Summary of Findings

The implementation is well-structured and follows the project's existing async patterns correctly. The module registration, `ChangelogError` type, `fetch_changelog` dispatch table, `BackendKind` storage, `packages_cache`, dialog UI, and security properties are all solid. However, two **critical** issues and four **moderate** issues require refinement before the feature is ready.

---

## Critical Issues

### CRITICAL-1 — APT backend uses `apt changelog` (network) instead of `apt-cache show` (offline)

**File:** `src/changelog.rs`, `fetch_apt()`  
**Severity:** Critical — correct behaviour on spec-mandated approach

The spec (§1.4, §4.1) explicitly evaluated `apt changelog` and **rejected it**:

> "`apt changelog <pkg>` — contacts `changelogs.ubuntu.com` or Debian mirrors; requires internet and only works for packages with published `.changelog` files. Hangs indefinitely if the network is unavailable. **Not suitable** as a primary source."

The spec mandates `apt-cache show --no-all-versions` (cap 20 packages, offline, always available from local package cache).

**What was implemented:**
```rust
// fetch_apt — WRONG: requires network, may hang up to 30 seconds per package
let text = match run_cmd("apt", &["changelog", pkg.as_str()]).await {
    Ok(t) => t,
    Err(_) => run_cmd("apt-get", &["changelog", pkg.as_str()]).await?,
};
// loops 3 times — potentially 90 seconds total timeout exposure
```

**What is required (from spec §4.1):**
```rust
tokio::process::Command::new("apt-cache")
    .args(["show", "--no-all-versions"])
    .args(&packages[..packages.len().min(20)])
    .output()
    .await
```

On Ubuntu/Debian (the primary APT users), this means the dialog may take up to 30 seconds to appear — or 90 seconds (3 packages × 30s) in the worst case — with zero user feedback. On systems without internet access, all three calls will time out. This is a regression compared to a completely offline approach.

---

### CRITICAL-2 — No loading indicator while changelog is fetching

**File:** `src/ui/update_row.rs`, changelog button click handler  
**Severity:** Critical — UX regression; UI appears frozen for up to 30–90 seconds

The spec (§2.1, §5 Step 3.6) explicitly requires:

> "Disables itself and shows a `gtk::Spinner` while fetching."  
> "Disable the button, show spinner."

The implementation fires the async task and immediately returns, with no UI change until the dialog appears. With `apt changelog` running 3 sequential network calls (each up to 30 seconds), the button appears unresponsive for the entire duration. Even for fast backends (Pacman, Zypper), there is no feedback.

**Required pattern (per spec):**
```rust
// Before spawning:
changelog_button.set_sensitive(false);
changelog_spinner.set_visible(true);
changelog_spinner.set_spinning(true);

// After dialog is closed or error shown:
changelog_button.set_sensitive(true);
changelog_spinner.set_visible(false);
changelog_spinner.set_spinning(false);
```

This also requires adding `changelog_spinner: gtk::Spinner` as a field in the struct (the spec lists it in §3.2 as a required new field — it is currently absent).

---

## Moderate Issues

### MODERATE-1 — Pacman and Zypper only query 1 package instead of up to 10

**File:** `src/changelog.rs`, `fetch_pacman()` and `fetch_zypper()`

Spec §4.3 and §4.4 pass `&packages[..packages.len().min(10)]` as multi-args, allowing info for up to 10 packages in a single command invocation. The implementation queries only `packages[0]`.

```rust
// Current (single package):
let output = run_cmd("pacman", &["-Si", packages[0].as_str()]).await?;

// Required (up to 10):
let pkgs: Vec<&str> = packages[..packages.len().min(10)]
    .iter().map(|s| s.as_str()).collect();
let mut args = vec!["-Si"];
args.extend(pkgs.iter().copied());
let output = run_cmd("pacman", &args).await?;
```

Same pattern applies to Zypper.

---

### MODERATE-2 — Homebrew does not use `--json=v2` flag or format output

**File:** `src/changelog.rs`, `fetch_homebrew()`

Spec §4.6 requires:
1. `brew info --json=v2 <pkg1> <pkg2> ...` (up to 10 packages)
2. Parse JSON to extract `formulae[].name`, `formulae[].desc`, `formulae[].homepage`
3. Format as structured human-readable blocks
4. Fall back to raw stdout on JSON parse failure

**Current implementation:**
```rust
let output = run_cmd("brew", &["info", packages[0].as_str()]).await?;
```

This uses human-readable `brew info` output for only 1 package and loses the structured JSON extraction.

---

### MODERATE-3 — fwupd called without `--json` flag

**File:** `src/changelog.rs`, `fetch_fwupd()`

Spec §4.7 requires `fwupdmgr get-updates --json` with JSON parsing of `Name`, `Version`, `Releases[0].Summary`, and `Releases[0].Description` per device. The implementation calls `fwupdmgr get-updates` (human-readable mode) and returns stdout verbatim.

The human-readable output is usable and the fwupd exit-code-2 handling is correctly implemented. However, the JSON approach is more reliable across fwupd versions and extracts more targeted information. This is a spec deviation.

---

## Minor Issues

### MINOR-1 — `ChangelogError::Exit` is a tuple variant, spec defines a struct variant

**File:** `src/changelog.rs`

```rust
// Implemented:
Exit(i32, String),
// Spec defined:
Exit { code: i32, message: String },
```

Both are functionally equivalent. The struct variant improves readability at match sites. No runtime impact.

---

### MINOR-2 — `ChangelogError::Empty` variant defined in spec but not implemented

The spec's `ChangelogError` enum includes an `Empty` variant. It is not referenced by the review criteria checklist but was defined in the spec for cases where output trims to nothing. Since none of the backend helpers return it, this is a documentation gap rather than a runtime bug.

---

### MINOR-3 — Dialog parent is the ExpanderRow widget, not the root window

**File:** `src/ui/update_row.rs`

```rust
// Implemented:
let parent = row_ref.upgrade();
dialog.present(parent.as_ref());
```

Spec §3.3 and §5 Step 3.6 recommend using `changelog_button.root().and_downcast::<gtk::Widget>()` so the dialog is parented to the top-level window. While libadwaita will walk up the widget hierarchy to find the window ancestor, using the direct root avoids potential edge cases if the widget is not yet attached.

---

### MINOR-4 — `crate::runtime::runtime().spawn()` used instead of `crate::ui::spawn_background_async`

**File:** `src/ui/update_row.rs`

The spec (§3.2, §5 Step 3.6) specifies using `crate::ui::spawn_background_async`. The implementation uses `crate::runtime::runtime().spawn()` directly. Both achieve the same result. This is functionally equivalent but deviates from the project convention and the spec.

---

## Criteria Checklist

### `src/changelog.rs`

| Criterion | Result |
|-----------|--------|
| `ChangelogError` thiserror-derived | ✅ |
| `NotSupported`, `Exit`, `Spawn` variants present | ✅ |
| `fetch_changelog` dispatches per-backend | ✅ |
| All commands use `tokio::process::Command` (no pkexec) | ✅ |
| `LANG=C` set on all invocations | ✅ (also sets `LC_ALL=C` — good) |
| 30-second timeout on all commands | ✅ |
| Output trimmed to 10,000 chars with `[...truncated]` suffix | ✅ |
| Non-zero exit → `Err(Exit(...))` | ✅ |
| `BackendKind::Nix` → `Err(NotSupported)` immediately | ✅ |
| fwupd exit code 2 → `Ok("No firmware updates available")` | ✅ |
| APT uses `apt-cache show` (offline, cap 20) | ❌ Uses `apt changelog` (network required, cap 3) |
| Pacman queries up to 10 packages | ❌ Queries only 1 package |
| Zypper queries up to 10 packages | ❌ Queries only 1 package |
| Homebrew uses `--json=v2` + formats output | ❌ Uses plain `brew info`, single package |
| fwupd uses `--json` and parses JSON | ❌ Uses human-readable mode |

### `src/ui/update_row.rs`

| Criterion | Result |
|-----------|--------|
| `backend_kind: BackendKind` field stored | ✅ |
| `changelog_row: adw::ActionRow` field present | ✅ |
| `packages_cache: Rc<RefCell<Vec<String>>>` field present | ✅ |
| `changelog_row` hidden initially | ✅ |
| Shown only when packages non-empty AND backend ≠ Nix | ✅ |
| `async_channel::bounded(1)` used in click handler | ✅ |
| `runtime().spawn()` used for background task | ✅ |
| `glib::spawn_future_local()` used for GTK main thread | ✅ |
| `Ok(text)` → `adw::AlertDialog` with `gtk::TextView` | ✅ |
| `Err(NotSupported)` → silent return | ✅ |
| `Err(other)` → error text in dialog | ✅ |
| `adw::AlertDialog` used (NOT `gtk::MessageDialog`) | ✅ |
| Text set via `buffer().set_text()` (not markup) | ✅ |
| `changelog_row` re-appended at bottom in `set_packages` | ✅ |
| `changelog_spinner: gtk::Spinner` field present | ❌ Missing |
| Button disabled + spinner shown during async fetch | ❌ No loading feedback |
| Button re-enabled after dialog closes | ❌ Not implemented |
| Uses `glib::clone!` with `#[weak]` for widget refs | ⚠️ Uses manual `downgrade()`/`upgrade()` (equivalent, not idiomatic) |

### `src/main.rs`

| Criterion | Result |
|-----------|--------|
| `mod changelog;` present | ✅ |

### `src/ui/window.rs`

| Criterion | Result |
|-----------|--------|
| `UpdateRow::new` call sites updated (no signature change required) | ✅ |

### Security

| Criterion | Result |
|-----------|--------|
| No changelog commands use `pkexec` | ✅ |
| Package names passed as separate `Command::arg()` calls | ✅ |
| `text_view.buffer().set_text()` used (not markup) | ✅ |
| No shell string interpolation | ✅ |

### No Regressions

| Criterion | Result |
|-----------|--------|
| `set_packages()` still works for non-changelog display | ✅ |
| `pkg_rows` children still populate as before | ✅ |
| `changelog_row` re-added at bottom after `set_packages` | ✅ |
| Existing `UpdateRow` constructor signature unchanged | ✅ |

---

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 62% | D+ |
| Best Practices | 82% | B |
| Functionality | 72% | C+ |
| Code Quality | 90% | A |
| Security | 97% | A+ |
| Performance | 75% | B- |
| Consistency | 85% | B+ |
| Build Success | 95% | A |

**Overall Grade: B- (82%)**

---

## Required Refinements (Priority Order)

### Must Fix — Critical

1. **[CRITICAL-1]** `fetch_apt`: Replace `apt changelog` with `apt-cache show --no-all-versions` capped at 20 packages. Remove the `apt-get changelog` fallback. Use a single multi-arg command invocation (not a per-package loop).

2. **[CRITICAL-2]** Add `changelog_spinner: gtk::Spinner` field to `UpdateRow`. In the button click handler: disable the button and show the spinner before spawning the async task. Re-enable the button and hide the spinner after the dialog is dismissed (or immediately after an error dialog is dismissed).

### Should Fix — Moderate

3. **[MODERATE-1]** `fetch_pacman`: Change to pass `&packages[..min(10)]` as multi-args rather than only `packages[0]`.

4. **[MODERATE-1]** `fetch_zypper`: Same — pass `&packages[..min(10)]` as multi-args.

5. **[MODERATE-2]** `fetch_homebrew`: Add `--json=v2` flag, query up to 10 packages, parse JSON output and format as structured blocks, fall back to raw stdout on parse failure.

6. **[MODERATE-3]** `fetch_fwupd`: Add `--json` flag and parse the JSON response to extract `Name`, `Version`, `Releases[0].Summary`, `Releases[0].Description` per device. Format as human-readable blocks.

### May Fix — Minor

7. **[MINOR-1]** Change `ChangelogError::Exit(i32, String)` to a struct variant `Exit { code: i32, message: String }`.

8. **[MINOR-3]** Use `changelog_button.root()` as the dialog parent instead of the ExpanderRow widget.

9. **[MINOR-4]** Use `crate::ui::spawn_background_async` (or `super::spawn_background_async`) instead of `crate::runtime::runtime().spawn()`.

---

## Verdict

**NEEDS_REFINEMENT**

The implementation has a strong foundation — the async architecture, security properties, module structure, and UI dialog are all correct. Two critical gaps must be addressed before shipping: the APT backend uses a network-dependent approach the spec explicitly rejected, and the missing loading indicator will cause the UI to appear frozen for potentially 30–90 seconds on APT-based systems.
