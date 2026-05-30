# Spec: Flatpak Check Bug — `--user` Scope Mismatch

**Feature name:** `flatpak_check_bug`  
**Date:** 2026-05-29  
**Status:** Research complete — awaiting implementation

---

## 1. Current State Analysis

### What the code does

The `FlatpakBackend` in `src/backends/flatpak.rs` has three distinct operations:

| Method | Command | Scope flag | Purpose |
|--------|---------|-----------|---------|
| `list_available()` | `flatpak remote-ls --updates --user --columns=application` | `--user` | **Check phase**: enumerate pending updates |
| `estimate_size()` | `flatpak remote-ls --updates --user --columns=download-size` | `--user` | **Check phase**: estimate download size |
| `run_update()` | `flatpak update -y` | *(none)* | **Update phase**: apply pending updates |

### The check pipeline

`src/check.rs` (`run_check()`) calls `backend.count_available()` for each detected backend.  
`count_available()` (default trait impl in `src/backends/mod.rs`) delegates to `list_available()`.  
`list_available()` runs `flatpak remote-ls --updates --user --columns=application`.

The UI path (`src/ui/window.rs` lines 564–565) also calls `count_available()` and `list_available()` to populate the update row badges.

### Flatpak installation scopes

Flatpak supports two installation paths on Linux:

| Scope | Default path | Typical use |
|-------|-------------|-------------|
| **User** | `~/.local/share/flatpak/` | Apps explicitly installed with `flatpak install --user` |
| **System** | `/var/lib/flatpak/` | Default install target on most distros (Fedora, openSUSE, Pop!_OS, etc.) |

On most mainstream Linux distributions, Flatpak applications are installed **system-wide by default** — `flatpak install` without a scope flag uses the system installation.

---

## 2. Root Cause

### Primary bug: `--user` scope restriction in `list_available()` and `estimate_size()`

**File:** `src/backends/flatpak.rs`, lines 121–128 (`list_available`) and lines 145–153 (`estimate_size`)

```rust
// CURRENT (broken) — list_available
let (cmd, args) =
    build_flatpak_cmd(&["remote-ls", "--updates", "--user", "--columns=application"]);
```

```rust
// CURRENT (broken) — estimate_size  
let (cmd, args) = build_flatpak_cmd(&[
    "remote-ls",
    "--updates",
    "--user",            // ← only user installation
    "--columns=download-size",
]);
```

```rust
// CURRENT (update) — run_update
let (cmd, args) = build_flatpak_cmd(&["update", "-y"]);
// ↑ No --user or --system: covers BOTH user and system installations
```

The `--user` flag restricts `remote-ls --updates` to **only** the user installation at `~/.local/share/flatpak/`. If the user has zero Flatpak apps in their user installation (they are all in `/var/lib/flatpak/`), `list_available()` returns an empty list and `count_available()` returns 0.

`run_update()` then runs `flatpak update -y` without any scope flag, which resolves and updates **all** installations (user + system). This is why the user sees 6 packages updated despite the check reporting zero.

### Why the misleading comment is wrong

The comment at line 124 states:

```
// The `--user` flag is intentional: the `--system` variant triggers a polkit
// prompt on every background check, which is poor UX.
```

This is incorrect. `flatpak remote-ls --updates` is a **read-only metadata query** — it does not acquire any locks, modify state, or require elevated privileges, regardless of whether `--user`, `--system`, or no scope flag is given. Polkit is only invoked by the actual *update* operation when modifying system-owned paths.

There is no UX or security reason to restrict the listing to `--user`. The `--user` flag was likely added defensively but produces a fundamentally incorrect result: the check reports fewer (or zero) updates than actually exist.

### Secondary issue: stale doc comment on `parse_flatpak_app_line`

The doc comment at line 261 says:

```rust
/// Parse a line from `flatpak update --no-deploy --columns=application` output.
```

The function is actually used to parse `flatpak remote-ls --updates --columns=application` output, not `flatpak update --no-deploy`. This is a staleness artifact suggesting the approach was changed at some point without updating the comment.

---

## 3. Proposed Fix

### Change 1 — Remove `--user` from `list_available()` (PRIMARY FIX)

**File:** `src/backends/flatpak.rs`

```rust
// BEFORE (lines 121–128)
// Use `flatpak remote-ls --updates --user --columns=application` to detect
// pending updates without applying them. The `--columns=application` flag
// ensures one application ID per line for predictable parsing.
// The `--user` flag is intentional: the `--system` variant triggers a polkit
// prompt on every background check, which is poor UX. System Flatpak installs
// are uncommon on desktop systems, so only user installations are checked here.
let (cmd, args) =
    build_flatpak_cmd(&["remote-ls", "--updates", "--user", "--columns=application"]);
```

```rust
// AFTER
// Use `flatpak remote-ls --updates --columns=application` to detect
// pending updates without applying them. The `--columns=application` flag
// ensures one application ID per line for predictable parsing.
// No --user or --system scope flag is passed so the query covers all
// installations (user + system), matching the scope of `run_update()` which
// also runs without a scope restriction.  This is a read-only metadata query
// and does not trigger polkit regardless of scope.
let (cmd, args) =
    build_flatpak_cmd(&["remote-ls", "--updates", "--columns=application"]);
```

### Change 2 — Remove `--user` from `estimate_size()` (CONSISTENCY FIX)

**File:** `src/backends/flatpak.rs`

```rust
// BEFORE (lines 145–153)
let (cmd, args) = build_flatpak_cmd(&[
    "remote-ls",
    "--updates",
    "--user",
    "--columns=download-size",
]);
```

```rust
// AFTER
let (cmd, args) = build_flatpak_cmd(&[
    "remote-ls",
    "--updates",
    "--columns=download-size",
]);
```

### Change 3 — Fix stale doc comment on `parse_flatpak_app_line`

**File:** `src/backends/flatpak.rs` (line 261)

```rust
// BEFORE
/// Parse a line from `flatpak update --no-deploy --columns=application` output.
/// Returns `Some(app_id)` for valid (non-empty, non-header) lines.
```

```rust
// AFTER
/// Parse a line from `flatpak remote-ls --updates --columns=application` output.
/// Returns `Some(app_id)` for valid (non-empty, non-header) lines.
```

### Change 4 — Fix stale doc comment in `disk.rs`

**File:** `src/disk.rs` (line 177)

```rust
// BEFORE
/// Parse `flatpak remote-ls --updates --user --columns=download-size` output.
```

```rust
// AFTER
/// Parse `flatpak remote-ls --updates --columns=download-size` output.
```

---

## 4. Files to Modify

| File | Lines affected | Change |
|------|---------------|--------|
| `src/backends/flatpak.rs` | 121–128 | Remove `--user` from `list_available()` args; update comment |
| `src/backends/flatpak.rs` | 145–153 | Remove `--user` from `estimate_size()` args |
| `src/backends/flatpak.rs` | 261 | Fix stale doc comment on `parse_flatpak_app_line` |
| `src/disk.rs` | 177 | Fix stale doc comment on `parse_flatpak_sizes` |

No other files require changes. The calling code in `src/check.rs`, `src/ui/window.rs`, and `src/backends/mod.rs` is correct and unaffected by this change.

---

## 5. Risks and Mitigations

### Risk 1: Polkit prompt during background check (low)

**Concern:** Removing `--user` might cause `flatpak remote-ls --updates` to trigger a polkit prompt when querying system installations.

**Mitigation:** `flatpak remote-ls` is documented as a **read-only** command. It reads OSTree metadata from disk; it does not acquire package manager locks or require elevated privileges. This risk is effectively zero. The original comment was incorrect.

**Verification:** Run `flatpak remote-ls --updates` (no flags) on a system with system-installed Flatpaks and confirm no polkit prompt appears.

### Risk 2: Duplicate entries if a package exists in both user and system installations (negligible)

**Concern:** An app installed in both user and system scopes might appear twice in the output.

**Mitigation:** `parse_flatpak_updates()` already deduplicates results (`if !apps.contains(&app_id)`), so any duplicates will be collapsed to a single entry. The count shown to the user will still be accurate.

### Risk 3: Behavior change on user-only systems (very low)

**Concern:** Users who have only user-installed Flatpaks and no system installation will see the same results as before (no regression).

**Mitigation:** Without a system installation, `remote-ls --updates` returns the same set of packages as `remote-ls --updates --user`. No behavior change for these users.

### Risk 4: System Flatpak updates may still require polkit during `run_update()` (existing, not new)

**Concern:** The update phase runs `flatpak update -y` without `pkexec`. On systems where system Flatpaks require polkit to update, the user may be prompted mid-update.

**Mitigation:** This is a pre-existing condition unrelated to this bug. `FlatpakBackend::needs_root()` returns `false`, which means the orchestrator does not pre-authenticate. If system Flatpak updates require polkit, `flatpak update -y` will handle the polkit prompt inline (via libflatpak's own polkit agent integration). This is consistent with current behavior and out of scope for this fix.

---

## 6. Implementation Steps

1. Edit `src/backends/flatpak.rs`:
   - In `list_available()`: remove `"--user"` from the `build_flatpak_cmd` call and update the preceding comment block.
   - In `estimate_size()`: remove `"--user"` from the `build_flatpak_cmd` call.
   - Fix the stale doc comment on `parse_flatpak_app_line`.

2. Edit `src/disk.rs`:
   - Fix the stale doc comment on `parse_flatpak_sizes`.

3. Verify existing tests still pass (`cargo test`). No new tests are required as the existing unit tests for `parse_flatpak_updates` and `run_update` remain valid.

4. Manual verification (recommended): On a system with system-installed Flatpaks with pending updates, confirm that `up --check` now reports the correct non-zero count.

---

## 7. Summary

| | Check (`list_available`) | Update (`run_update`) |
|--|--|--|
| **Before fix** | `flatpak remote-ls --updates --user` → user scope only | `flatpak update -y` → all scopes |
| **After fix** | `flatpak remote-ls --updates` → all scopes | `flatpak update -y` → all scopes |

The one-line root cause: **`--user` in `list_available()` restricts the check to user-installed Flatpaks only, while `run_update()` updates all Flatpak installations. Users with system-installed Flatpaks (the default on most distributions) will always see "no updates" from the check even when real updates exist.**
