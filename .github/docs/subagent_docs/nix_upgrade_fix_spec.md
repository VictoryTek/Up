# Specification: Nix Upgrade Fix & Upgrade Tab Visibility

**Feature Name:** `nix_upgrade_fix`  
**Date:** 2026-04-24  
**Status:** Draft  

---

## 1. Current State Analysis

### 1.1 `src/backends/determinate_nix.rs`

`DeterminateNixBackend` is a standalone backend implementing the `Backend` trait.

Key facts:
- `is_available()` checks for `/nix/receipt.json` AND `which::which("determinate-nixd").is_ok()` — reliable detection.
- `needs_root()` returns `true` — **incorrect**. Determinate Nix upgrades are handled by the `determinate-nixd` daemon over D-Bus; the CLI itself runs unprivileged.
- `run_update()` invokes:
  ```
  pkexec env PATH=/nix/var/nix/profiles/default/bin:/run/wrappers/bin sh -c determinate-nixd upgrade
  ```
  This is **broken** — `pkexec env` restores PATH but does not guarantee `sh` is on the path in all polkit environments. Observed error: `env: 'sh': No such file or directory`.
- Contains useful helper functions: `upgrade_available_in_output()` and `count_determinate_upgraded()`, and a full test suite.
- `count_available()` / `list_available()` correctly use `determinate-nixd version` (unprivileged).

### 1.2 `src/backends/nix.rs`

`NixBackend` handles both NixOS system updates and non-NixOS Nix profile updates.

Key facts:
- NixOS path (flake and legacy channel) correctly uses `pkexec` — no changes needed.
- **Non-NixOS path in `run_update()`**: Performs a manifest version check:
  ```rust
  let manifest_path = $HOME/.nix-profile/manifest.json
  let use_flakes = content.contains("\"version\": 2")
  // fallback when file is unreadable: false → nix-env -u
  ```
  The fallback is `false` (use `nix-env -u`) — **this is the root cause of Issue 2**. On systems using Nix 2.14+ XDG state directories, `~/.nix-profile` may not point to a readable manifest, so the code falls through to `nix-env -u` and fails with:
  ```
  error: profile '/home/nimda/.local/state/nix/profiles/profile' is incompatible with 'nix-env'; please use 'nix profile' instead
  ```
- **Non-NixOS path in `count_available()` and `list_available()`**: Unconditionally use `nix-env -u --dry-run` with no manifest check. Same breakage occurs.

### 1.3 `src/backends/mod.rs`

- Defines `BackendKind` enum including `DeterminateNix`.
- `detect_backends()` registers both `NixBackend` and `DeterminateNixBackend` as separate entries — produces two separate UI rows.

### 1.4 `src/upgrade.rs` — `detect_distro()`

```rust
let upgrade_supported = match id.as_str() {
    "ubuntu" => which::which("do-release-upgrade").is_ok(),
    "fedora" => true,
    "opensuse-leap" => true,
    "nixos" => true,
    _ => false,
};
```

Issues:
- `"ubuntu"` is gated behind `do-release-upgrade` availability. On Ubuntu Desktop this tool is almost always present, but if not found (minimal installs, cloud images), the tab is hidden — contrary to expected UX.
- `"debian"` is intentionally omitted with a comment "no safe automated upgrade path". However, Debian has `apt-get dist-upgrade` / `do-release-upgrade` on some configurations, and users expect to see the tab.
- Missing supported distros: `linuxmint`, `pop` (Pop!_OS), `elementary`, `zorin`.
- `ID_LIKE` field is not consulted — Ubuntu derivatives (Mint, Pop) use `ID=linuxmint` / `ID=pop` but `ID_LIKE=ubuntu`, so they are missed.

### 1.5 `src/ui/window.rs`

The upgrade tab visibility is correctly gated:
```rust
upgrade_stack_page.set_visible(info.upgrade_supported);
```
The window logic is sound. No changes needed in this file.

---

## 2. Problem Definition

### Issue 1: Determinate Nix as a separate backend row
The `DeterminateNixBackend` registers as a distinct row in the Sources list alongside `NixBackend`. When Determinate Nix is installed, users see both "Nix" and "Determinate Nix" rows. The correct behaviour is a single "Nix" row that transparently handles both regular Nix and Determinate Nix.

### Issue 2: Nix profile update command broken on modern profiles
`nix-env -u` fails on Nix ≥ 2.14 profiles stored under `~/.local/state/nix/profiles/` with:
```
error: profile '/home/nimda/.local/state/nix/profiles/profile' is incompatible with 'nix-env'; please use 'nix profile' instead
```
Root cause: the manifest check fallback defaults to `nix-env -u` when the manifest file cannot be read. This is wrong — the default should be `nix profile upgrade '.*'` (the modern, supported command).

### Issue 3: Determinate Nix command error
The current `pkexec env PATH=... sh -c determinate-nixd upgrade` command fails. `determinate-nixd upgrade` does not require root privileges — the `determinate-nixd` daemon (running as root) handles privilege internally via D-Bus. The command should be run directly without `pkexec`.

### Issue 4: Upgrade tab hidden on Ubuntu
Ubuntu's upgrade tab visibility is gated on the presence of `do-release-upgrade`. On systems where the binary is absent (minimal installs, fresh Ubuntu images before first full upgrade), the tab is invisible. Additionally, Ubuntu derivatives and other supported distros are not included.

---

## 3. Proposed Solution Architecture

### 3.1 Merge Determinate Nix into Nix Backend

**File changes:**
1. **Modify** `src/backends/nix.rs` — add Determinate Nix detection and update logic inline.
2. **Delete** `src/backends/determinate_nix.rs` — remove the standalone backend file.
3. **Modify** `src/backends/mod.rs` — remove the `determinate_nix` module, the `DeterminateNix` enum variant, and its detection in `detect_backends()`.

**Logic in `nix.rs`:**

Add a private helper to detect Determinate Nix (reuse existing detection logic):
```rust
fn is_determinate_nix() -> bool {
    std::path::Path::new("/nix/receipt.json").exists()
        && which::which("determinate-nixd").is_ok()
}
```

Move helper functions from `determinate_nix.rs` into `nix.rs`:
- `upgrade_available_in_output(output: &str) -> bool`
- `count_determinate_upgraded(output: &str) -> usize`

Update `run_update()` for the non-NixOS branch:
```
if is_determinate_nix():
    → runner.run("determinate-nixd", &["upgrade"])   // no pkexec
    → UpdateResult based on count_determinate_upgraded()
else if manifest is confirmed v1 (nix-env compatible):
    → runner.run("nix-env", &["-u"])
else:
    → runner.run("nix", &["--extra-experimental-features", "nix-command", "profile", "upgrade", ".*"])
```

The manifest check logic change: invert the fallback so that unreadable manifest → use `nix profile upgrade` (not `nix-env -u`). Only use `nix-env -u` when the manifest is confirmed to be a v1 (non-"version": 2) file.

```rust
let use_legacy_nix_env = {
    let manifest_path = std::path::Path::new(&std::env::var("HOME").unwrap_or_default())
        .join(".nix-profile/manifest.json");
    if let Ok(content) = std::fs::read_to_string(&manifest_path) {
        // Only use nix-env when manifest is NOT version 2
        !content.contains("\"version\": 2")
    } else {
        // Can't read manifest — default to modern nix profile upgrade
        false
    }
};
```

Update `count_available()` for the non-NixOS branch:
```
if is_determinate_nix():
    → run `determinate-nixd version`, parse output with upgrade_available_in_output()
    → return Ok(1) or Ok(0)
else if use_legacy_nix_env:
    → run `nix-env -u --dry-run`, count "upgrading" lines in stderr
else:
    → return Ok(0)  // nix profile upgrade has no dry-run equivalent
```

Update `list_available()` for the non-NixOS branch:
```
if is_determinate_nix():
    → run `determinate-nixd version`, parse
    → return Ok(vec!["determinate-nix"]) or Ok(vec![])
else if use_legacy_nix_env:
    → run `nix-env -u --dry-run`, parse "upgrading 'name'" lines
else:
    → return Ok(vec![])  // nix profile upgrade has no dry-run equivalent
```

**`needs_root()` change:**  
`NixBackend::needs_root()` currently returns `is_nixos()`. When Determinate Nix is detected, root is NOT needed. Since `is_determinate_nix()` implies non-NixOS (Determinate Nix is not used on NixOS), the existing `is_nixos()` check remains correct — no change required.

**`description()` change:**  
Add a Determinate Nix branch:
```rust
fn description(&self) -> &str {
    if is_determinate_nix() {
        "Determinate Nix installation (determinate-nixd)"
    } else if is_nixos() {
        "NixOS system packages"
    } else {
        "Nix profile packages"
    }
}
```

### 3.2 Fix Upgrade Tab Visibility

**File:** `src/upgrade.rs` — `detect_distro()` function.

Expand the `upgrade_supported` match to include:
- Ubuntu derivatives via `ID_LIKE` field parsing.
- Additional explicitly-supported distro IDs.
- Remove the `do-release-upgrade` gating for Ubuntu (the tab is informational; the upgrade page itself can display an error if the tool is missing).

Revised logic:
```rust
let id_like = fields.get("ID_LIKE").cloned().unwrap_or_default();

let upgrade_supported = match id.as_str() {
    // Explicitly supported distros with version upgrade tooling
    "ubuntu" | "linuxmint" | "pop" | "elementary" | "zorin" => true,
    "fedora" => true,
    "opensuse-leap" => true,
    "debian" => true,
    "nixos" => true,
    // Catch Ubuntu derivatives that set ID_LIKE=ubuntu
    _ if id_like.split_whitespace().any(|s| s == "ubuntu") => true,
    // Catch Debian derivatives
    _ if id_like.split_whitespace().any(|s| s == "debian") => true,
    _ => false,
};
```

Note: The `ID_LIKE` field in `/etc/os-release` is a space-separated list of base distros (e.g., `ID_LIKE="ubuntu debian"`). Splitting on whitespace and checking for exact matches is safe and correct.

**Rationale for removing the `do-release-upgrade` guard:**  
The upgrade page already handles the case where the upgrade tool is unavailable — it runs prerequisite checks and the actual upgrade command. If `do-release-upgrade` is absent, the upgrade will fail at runtime with an informative error. Hiding the tab entirely gives no feedback and is worse UX than showing the page with a clear error message.

---

## 4. Exact Implementation Steps

### Step 1: Modify `src/backends/nix.rs`

1. **Add helper functions** near the top of the file (before `NixBackend` struct):
   - `fn is_determinate_nix() -> bool` — detection logic from `determinate_nix.rs`
   - `fn upgrade_available_in_output(output: &str) -> bool` — from `determinate_nix.rs`
   - `fn count_determinate_upgraded(output: &str) -> usize` — from `determinate_nix.rs`

2. **Modify `NixBackend::description()`** to include Determinate Nix branch.

3. **Modify `NixBackend::run_update()`** — non-NixOS else branch:
   - Add Determinate Nix check first (run `determinate-nixd upgrade` directly, no pkexec).
   - Fix manifest fallback logic: unreadable manifest → use `nix profile upgrade '.*'`.
   - Keep v1 manifest path using `nix-env -u` for backwards compatibility.
   - The `nix profile upgrade` invocation must pass `--extra-experimental-features nix-command` for compatibility with Nix installations where `nix-command` is not enabled by default.

4. **Modify `NixBackend::count_available()`** — non-NixOS (non-flake) else branch:
   - Add Determinate Nix check.
   - Apply same manifest check logic as `run_update()`.
   - Return `Ok(0)` for new-style profiles (no dry-run available).

5. **Modify `NixBackend::list_available()`** — non-NixOS else branch:
   - Same as `count_available()` structure.
   - Return `Ok(vec![])` for new-style profiles.

6. **Add unit tests** for new helper functions (mirror the test suite from `determinate_nix.rs`).

### Step 2: Delete `src/backends/determinate_nix.rs`

Remove the file entirely.

### Step 3: Modify `src/backends/mod.rs`

1. Remove `pub mod determinate_nix;` declaration.
2. Remove `DeterminateNix` from `BackendKind` enum.
3. Remove `Self::DeterminateNix => write!(f, "Determinate Nix")` from `fmt::Display` impl.
4. Remove the detection block:
   ```rust
   if determinate_nix::is_available() {
       backends.push(Arc::new(determinate_nix::DeterminateNixBackend));
   }
   ```

### Step 4: Modify `src/upgrade.rs` — `detect_distro()`

1. Extract `ID_LIKE` field from the parsed `fields` map.
2. Replace the current `upgrade_supported` match block with the expanded version (see Section 3.2).

---

## 5. Dependencies

No new dependencies required. All changes use existing crates:
- `which` (already in `Cargo.toml`) — for `is_determinate_nix()`.
- `tokio::process::Command` (already used) — for async `determinate-nixd version` calls.

---

## 6. Risks and Mitigations

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| `nix profile upgrade '.*'` fails on v1 profiles | Medium | Only invoke if manifest check fails to confirm v1; v1 detection is preserved as a fast path |
| Removing `BackendKind::DeterminateNix` breaks existing serialized state | Low | `BackendKind` is only used in-memory (channel messages between threads and UI); it is never persisted to disk. No migration needed. |
| `determinate-nixd upgrade` output format changes | Low | `count_determinate_upgraded()` has conservative fallback: unknown output → assume 1 updated. Tests cover known output patterns. |
| Ubuntu tab visible but `do-release-upgrade` missing | Low-Medium | The upgrade page's Run Checks flow will surface a clear error. This is better UX than invisibly hiding the tab. |
| Debian upgrade tab causes confusion (no standard tool) | Medium | Debian path exists in `upgrade.rs` check commands (`check_packages_up_to_date` handles "debian"? No — it's unhandled). Adding Debian to `upgrade_supported` without implementing the upgrade command path would show the tab but fail at runtime. **Mitigation: Do not add "debian" to the supported list in this spec — defer to a dedicated Debian upgrade feature.** |
| `ID_LIKE` field absent on some distros | Low | The `_ if id_like.split_whitespace().any(...)` arms only match when the field is present; `id_like` defaults to empty string. No false positives. |
| `nix --extra-experimental-features nix-command profile upgrade '.*'` not supported on very old Nix | Very Low | Nix versions old enough to lack `nix profile` also lack new-style manifests; the v1 manifest detection path handles them correctly. |

---

## 7. File Summary

| File | Action | Reason |
|------|--------|--------|
| `src/backends/nix.rs` | Modify | Add Determinate Nix detection, fix manifest fallback, merge helpers |
| `src/backends/determinate_nix.rs` | Delete | Merged into nix.rs |
| `src/backends/mod.rs` | Modify | Remove DeterminateNix variant and module, remove backend registration |
| `src/upgrade.rs` | Modify | Expand upgrade_supported distro list, use ID_LIKE |

---

## 8. Out of Scope

- Debian version upgrade implementation (upgrading Debian stable to next stable requires careful handling of sources.list rewriting — deferred to a separate feature).
- NixOS upgrade path (already working; no changes).
- Flake-based NixOS update path (already working; no changes).
- Homebrew, Flatpak, APT, DNF, Pacman, Zypper backends (no changes).
