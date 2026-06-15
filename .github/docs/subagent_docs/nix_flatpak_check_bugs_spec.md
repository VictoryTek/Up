# Spec: Fix NixOS (VexOS) False Positive and Flatpak False Negative in `list_available`

**Feature name:** `nix_flatpak_check_bugs`
**Date:** 2026-06-14
**Status:** Ready for implementation

---

## 1. Current State Analysis

### 1.1 Bug 1 — NixOS (VexOS) always shows updates

`NixBackend::list_available()` in `src/backends/nix.rs` (lines 619–629):

```rust
if is_nixos() && is_nixos_flake() {
    if is_vexos() {
        // Always indicates update available.
        Ok(vec!["NixOS system".to_string()])
    } else {
        nixos_flake_changed_inputs().await
    }
}
```

The VexOS branch unconditionally returns `["NixOS system"]`, so the UI always
shows 1 update available for VexOS systems regardless of whether any flake
inputs have changed upstream. This is a false positive every time the user
checks and all inputs are already current.

The original comment said "we cannot determine if a rebuild is needed without
running the command", but `nixos_flake_changed_inputs()` — the same function
used for standard NixOS flake — does exactly that check by comparing flake.lock
revisions before and after a dry update. It is already called for standard
(non-VexOS) NixOS flake systems. VexOS has `/etc/nixos/flake.nix` and
`/etc/nixos/flake.lock`, so `nixos_flake_changed_inputs()` works identically.

### 1.2 Bug 2 — Flatpak always shows up to date

`flatpak_remote_ls_updates()` in `src/backends/flatpak.rs` (lines 257–273):

```rust
async fn flatpak_remote_ls_updates(scope: &str) -> Result<Vec<String>, String> {
    let (cmd, args) =
        build_flatpak_cmd(&["remote-ls", "--updates", scope, "--columns=application"]);
    // ...
    Ok(parse_flatpak_updates(&text))
}
```

`--columns=application` restricts the output to the "application" column.
Flatpak **runtimes** (e.g., `org.gnome.Platform`, `org.kde.Platform`,
`org.freedesktop.Platform`) have an empty value in the "application" column —
only true applications (user-facing apps) have a non-empty value there.

`parse_flatpak_app_line` already filters empty lines:
```rust
if !t.is_empty() && !t.eq_ignore_ascii_case("application") { ... }
```

So runtime updates appear as blank lines in `--columns=application` output and
are silently dropped. The UI shows "Up to date" even when `flatpak update -y`
would actually update several runtimes.

Runtime updates are frequent on Flatpak systems (GNOME Platform, KDE Frameworks,
etc. are updated regularly). On VexOS/NixOS where system-installed Flatpak apps
depend on these runtimes, the check consistently undercounts available updates.

### 1.3 Secondary issue — silent false negative when one scope errors

`list_available()` (lines 125–142):

```rust
match (user_result, system_result) {
    (Ok(user_pkgs), Ok(sys_pkgs)) => { /* merge */ }
    (Ok(pkgs), Err(_)) | (Err(_), Ok(pkgs)) => Ok(pkgs),
    (Err(e), Err(_)) => Err(e),
}
```

When system scope fails (`Err`) and user scope returns `Ok([])` (empty, because
no user-installed apps), the code returns `Ok([])`. `count_available()` returns
`Ok(0)`, the row shows "Up to date", and no error is visible. This masks system
scope failures entirely.

### 1.4 Secondary issue — headline "Everything is up to date." ignores check errors

`window.rs` (lines 709–727):

```rust
if remaining == 0 {
    let non_skipped_total: usize = {
        rows.borrow().iter()
            .filter(|(_, r)| !r.is_skipped())
            .filter_map(|(_, r)| r.last_available_count())
            .sum()
    };
    if non_skipped_total > 0 { ... }
    else { status_label_checks.set_label("Everything is up to date."); }
}
```

`last_available_count()` returns `None` for both "not yet checked" and "check
errored" states. `filter_map` silently drops `None`, making an errored backend
contribute 0 to the total. If all other backends are up to date and Flatpak
errors, the headline reads "Everything is up to date." — a false positive.

`UpdateRow` has no `check_errored` field: `set_status_unknown()` does not record
the error state, so the headline logic cannot distinguish between "0 updates"
and "check failed."

---

## 2. Problem Definitions

### Bug 1: VexOS false positive
- `is_vexos()` → always returns `["NixOS system"]` → always shows 1 update
- Real behaviour: 0 updates when upstream flake inputs haven't changed

### Bug 2: Flatpak false negative (primary)
- `--columns=application` → runtime refs have empty application column → filtered
- Flatpak update -y detects and installs runtime updates that list_available misses

### Bug 2b: Flatpak false negative (secondary)
- System scope error + empty user scope → silently returns `Ok([])`
- No error visible; headline says "Everything is up to date."

### Bug 2c: Headline false positive when errors exist
- `set_status_unknown()` sets no error flag
- Headline can claim "Everything is up to date." when a backend errored

---

## 3. Proposed Solution Architecture

### Fix 1 — VexOS: delegate to `nixos_flake_changed_inputs()`

In `NixBackend::list_available()`, replace the VexOS unconditional return with:

```rust
if is_vexos() {
    match nixos_flake_changed_inputs().await {
        Ok(inputs) if inputs.is_empty() => Ok(vec![]),
        Ok(_) => Ok(vec!["NixOS system".to_string()]),
        Err(e) => Err(e),
    }
}
```

When no flake inputs have changed: `Ok([])` → UI shows "Up to date."
When inputs changed: `Ok(["NixOS system"])` → UI shows "1 update available."
When check errors: `Err(e)` → UI shows the error message.

This is the same detection logic used for standard NixOS flake systems.
`supports_item_selection()` already excludes VexOS, so per-item selection is
unaffected. `run_update()` for VexOS is unchanged — it always runs
`vexos-update` when the user clicks Update.

### Fix 2 — Flatpak: use `--columns=name` instead of `--columns=application`

Replace `--columns=application` with `--columns=name` in `flatpak_remote_ls_updates`.

The "name" column is the ref's application/runtime identifier. For apps it is
the app ID (e.g., `com.google.Chrome`); for runtimes it is the runtime ID
(e.g., `org.gnome.Platform`). Both are non-empty and meaningful.

Update `parse_flatpak_app_line` to filter the "Name" column header in addition
to "Application" (for forward compatibility when the function is called with
either column output).

### Fix 3 — Flatpak: surface system-scope errors

Replace:
```rust
(Ok(pkgs), Err(_)) | (Err(_), Ok(pkgs)) => Ok(pkgs),
```
With:
```rust
(Ok(pkgs), Err(e)) => if pkgs.is_empty() { Err(e) } else { Ok(pkgs) },
(Err(e), Ok(pkgs)) => if pkgs.is_empty() { Err(e) } else { Ok(pkgs) },
```

When one scope finds updates, return them even if the other scope failed.
When the only successful scope returned empty, surface the failure error.
This prevents false "Up to date" on systems where the system scope errors and
the user scope has no apps.

### Fix 4 — `UpdateRow`: add `check_errored` flag

Add `check_errored: Rc<Cell<bool>>` to `UpdateRow`:
- `set_status_checking()`: reset to `false`
- `set_status_unknown()`: set to `true`
- New method `has_check_error() -> bool`: read the flag

### Fix 5 — `window.rs`: accurate headline when errors exist

After the `remaining == 0` block computes `non_skipped_total`, also check
whether any non-skipped row has a check error:

```rust
let any_check_error = {
    let borrowed = rows.borrow();
    borrowed.iter()
        .filter(|(_, r)| !r.is_skipped())
        .any(|(_, r)| r.has_check_error())
};
if non_skipped_total > 0 {
    // existing: show "N updates available"
} else if any_check_error {
    status_label_checks.set_label("Could not check all sources.");
} else {
    status_label_checks.set_label("Everything is up to date.");
}
```

---

## 4. Files to Modify

| File | Change |
|------|--------|
| `src/backends/nix.rs` | VexOS `list_available` branch: delegate to `nixos_flake_changed_inputs()` |
| `src/backends/flatpak.rs` | `flatpak_remote_ls_updates`: `--columns=name`; parse header fix; error handling fix |
| `src/ui/update_row.rs` | Add `check_errored` field, `has_check_error()`, update `set_status_checking()` and `set_status_unknown()` |
| `src/ui/window.rs` | Add `any_check_error` check + `else if` branch in headline logic |

No new dependencies. No schema changes. No Cargo.toml changes.

---

## 5. Implementation Steps

1. `src/backends/nix.rs`:
   - Replace VexOS branch return with `nixos_flake_changed_inputs()` delegation

2. `src/backends/flatpak.rs`:
   - In `flatpak_remote_ls_updates`: change `"--columns=application"` → `"--columns=name"`
   - In `parse_flatpak_app_line`: add `&& !t.eq_ignore_ascii_case("name")` to the filter
   - In `list_available` match: replace the `(Ok(pkgs), Err(_)) | (Err(_), Ok(pkgs))` arm
   - Update `parse_flatpak_app_line` doc comment to reflect `name` column

3. `src/ui/update_row.rs`:
   - Add `check_errored: Rc<Cell<bool>>` field to `UpdateRow` struct
   - Initialise `check_errored: Rc::new(Cell::new(false))` in `Self { ... }`
   - In `set_status_checking()`: add `self.check_errored.set(false);`
   - In `set_status_unknown()`: add `self.check_errored.set(true);`
   - Add `pub fn has_check_error(&self) -> bool { self.check_errored.get() }`

4. `src/ui/window.rs`:
   - After the `non_skipped_total` block, add `any_check_error` computation
   - Add `else if any_check_error` branch before the existing `else`

5. Update tests in `src/backends/flatpak.rs`:
   - `test_parse_flatpak_app_line_header_skipped`: add "Name" and "name" cases
   - `test_parse_flatpak_updates_happy_path`: update expected output (now uses "Name" header)
   - `test_parse_flatpak_updates_only_header`: test with "Name" header

---

## 6. Risks and Mitigations

| Risk | Mitigation |
|------|-----------|
| VexOS: `nixos_flake_changed_inputs()` requires network; slow check | Same latency as standard NixOS flake check; acceptable |
| VexOS: check errors → row shows error, not "Up to date" | Correct; error message is more honest than silent false positive |
| Flatpak: `--columns=name` output format differs across versions | `name` column is stable in Flatpak ≥ 0.11; all current distributions ship ≥ 1.x |
| Flatpak: runtime IDs shown in package list (e.g., org.gnome.Platform) | Acceptable; runtimes are real pending updates; user sees them in the expander |
| Error handling change: more errors surface for users with broken system remotes | Better than silently hiding failures; user sees actionable error message |

---

## 7. Build & Validation Commands

- `cargo build` — must compile without errors
- `cargo clippy -- -D warnings` — must produce no warnings
- `cargo fmt --check` — must pass
- `cargo test` — all existing tests must pass; updated tests must pass
