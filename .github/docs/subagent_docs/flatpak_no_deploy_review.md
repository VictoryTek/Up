# Review: Flatpak `list_available` — `--no-deploy` Implementation

**Date:** 2026-04-21  
**Spec:** `.github/docs/subagent_docs/flatpak_no_deploy_spec.md`  
**Reviewed file:** `src/backends/flatpak.rs`  
**Verdict:** NEEDS_REFINEMENT

---

## 1. Build Validation Results

| Command | Exit Code | Result |
|---------|-----------|--------|
| `cargo fmt --check` | 1 | ❌ **FAILED** — formatting diff in `flatpak.rs` line 298 |
| `cargo clippy -- -D warnings` | 101 | ⚠️ Env failure — `pkg-config` not found on host |
| `cargo build` | 101 | ⚠️ Env failure — `pkg-config` not found on host |
| `cargo test` | 101 | ⚠️ Env failure — `pkg-config` not found on host |

### `cargo fmt --check` failure — CRITICAL

Rustfmt requires the `let (cmd, args) =` binding to be on a single line.
The implementation writes it across two lines:

```rust
// Current (FAILS fmt --check)
let (cmd, args) =
    build_flatpak_cmd(&["update", "--no-deploy", "-y", "--user"]);

// Required by rustfmt
let (cmd, args) = build_flatpak_cmd(&["update", "--no-deploy", "-y", "--user"]);
```

The combined line is ≈ 91 characters, within rustfmt's default 100-character
limit. No wrapping is necessary.

### Environment failures (clippy / build / test)

Commands 2–4 failed because `pkg-config` is absent from the review
environment, preventing the GTK4/GLib `-sys` build scripts from locating
their system libraries. This is an **infrastructure constraint**, not a code
defect. The Rust code itself is otherwise structurally sound for compilation.

---

## 2. Code Review Findings

### 2.1 Correctness — PASS

`flatpak update --no-deploy -y --user` is the correct replacement for
`flatpak remote-ls --updates`. Key improvements over the previous approach:

- Uses Flatpak's **full transaction resolver** (same engine as a real update),
  so pinned refs, end-of-life rebases, and dependency deduplication are all
  handled correctly.
- Eliminates the false-positive category where `remote-ls` reported updates
  that `flatpak update` resolved to "Nothing to do."
- The `--dry-run` note in the original code was accurate: that flag was removed
  in Flatpak 1.16; `--no-deploy` is the supported equivalent.

### 2.2 Output Parsing — PASS

The digit + `ends_with('.')` heuristic is sound:

| Line type | Trimmed example | Digit-starts | Ends with `.` | Parsed? |
|-----------|-----------------|:---:|:---:|:---:|
| Header row | `"ID   Arch   Branch"` | No | — | ✗ (correct) |
| Update row | `"1.   com.example.App   …"` | Yes | Yes | ✓ (correct) |
| Progress % | `"99%  5.0 MB / 5.0 MB"` | Yes | No | ✗ (correct) |
| Info text | `"Nothing to do."` | No | — | ✗ (correct) |
| `"Updates complete."` | No | — | ✗ (correct) |

The progress-placeholder column (three spaces) collapses to nothing under
`split_whitespace()`, so **token 1 is reliably the app ID** — consistent with
the spec's whitespace-analysis in Section 5.

The `apps.contains()` O(n) dedup is appropriate; update lists are small
(typically < 50 apps).

### 2.3 Edge Cases — PASS (with one documented limitation)

| Case | Behaviour | Assessment |
|------|-----------|------------|
| "Nothing to do." output | No digit lines → `Ok(vec![])` | ✓ Correct |
| Empty stdout | Same as above | ✓ Correct |
| Command not found (e.g. no `flatpak`) | `map_err(|e| e.to_string())` propagates OS error | ✓ Correct |
| Network failure / non-zero exit | Parses partial/empty stdout; returns `Ok(vec![])` | ⚠️ See §2.3.1 |
| Inside Flatpak sandbox | `build_flatpak_cmd` wraps as `flatpak-spawn --host …` | ✓ Correct |
| App IDs with unusual chars | Flatpak IDs never contain whitespace; safe | ✓ Correct |
| Very large update lists | UI caps at 50; no parsing concern | ✓ Correct |

#### 2.3.1 Non-zero exit code not checked (Recommended improvement)

`tokio::process::Command::output()` captures stdout/stderr regardless of exit
code. If `--no-deploy` fails due to a network error, the exit code is non-zero
but the code silently returns an empty list, which the UI renders as "no
updates available." This is misleading.

The spec identifies this as a recommended improvement (Section 6):
> "Consider mapping non-zero exit to `Err(stderr)` for better diagnostics."

**This is not blocking** (the spec explicitly calls it non-critical), but it
should be addressed in a follow-up or the current refinement pass.

### 2.4 Consistency with `run_update()` — PASS

`run_update()` uses the digit-start heuristic to count updated packages from
`flatpak update -y` output. The output format of `flatpak update -y` and
`flatpak update --no-deploy -y` is identical up to the deployment step, so the
heuristics are compatible. The implementation correctly does NOT change
`run_update()`.

`list_available()` intentionally uses `tokio::process::Command` directly
(bypassing `runner.run()` / `PrivilegedShell`) — this is deliberate, matching
the previous `remote-ls` approach: no privilege escalation for a background
check.

### 2.5 Security — PASS

- All arguments are passed as `Vec<String>` elements, never shell-interpolated.
- No `unsafe` code.
- The URL validation in `download_and_install_bundle()` (prefix check +
  single-quote rejection) is unchanged and correct.
- No new attack surface introduced by this change.

### 2.6 UX / Known Limitation — DOCUMENTED

**System Flatpak installations are not checked.**

`--user` covers `~/.local/share/flatpak/` (Flathub default on GNOME). System
installations (`/var/lib/flatpak/`, managed by the distro or admin) require
polkit authentication for `--no-deploy`, making them unsuitable for a silent
background check. The code comment documents this explicitly:

> "The `--user` flag is intentional: the `--system` variant triggers a polkit
> prompt on every background check, which is poor UX."

This is a **known, intentional limitation** — not a defect. The previous
`remote-ls` implementation included a system check but its false-positive rate
undermined its value anyway. The trade-off is acceptable.

---

## 3. Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 92% | A− |
| Best Practices | 85% | B |
| Functionality | 95% | A |
| Code Quality | 80% | B− |
| Security | 100% | A+ |
| Performance | 90% | A− |
| Consistency | 95% | A |
| Build Success | 25% | F* |

**Overall Grade: B− (83%)**

> \* Build Success is graded F due to the `cargo fmt --check` failure (a real
> code defect). The other build failures (clippy, build, test) are attributed
> to a missing `pkg-config` on the review host (environment constraint), not
> to a code defect, and are noted separately.

---

## 4. Issues Summary

### CRITICAL (blocks merge)

1. **`cargo fmt --check` fails** — `let (cmd, args) =` binding in
   `list_available()` is formatted across two lines. Must be joined to a
   single line to pass rustfmt.

   **File:** `src/backends/flatpak.rs`, `list_available()` (line ~298–299)  
   **Fix:** `let (cmd, args) = build_flatpak_cmd(&["update", "--no-deploy", "-y", "--user"]);`

### RECOMMENDED (non-blocking)

2. **Non-zero exit code silently ignored** — When `flatpak update --no-deploy`
   fails (e.g. network error), the code returns `Ok(vec![])` instead of
   propagating an error. Consider checking `out.status.success()` and
   returning `Err(stderr)` on failure for better diagnostic visibility.

---

## 5. Verdict

**NEEDS_REFINEMENT**

One critical issue must be resolved before this change is merge-ready:
the `cargo fmt --check` failure caused by a two-line `let` binding that
rustfmt requires on a single line. The fix is trivial (one line change).
The recommended improvement (exit code propagation) may be addressed in
the same pass.
