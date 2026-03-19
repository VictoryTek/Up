# Security Fix: Overly Broad Flatpak Permissions

**Type:** Security — MEDIUM severity (two related findings)
**Findings:**
- Finding #2 — Overly Broad `--filesystem=host:ro`
- Finding #3 — Unused D-Bus Permission `org.freedesktop.PackageKit`
**Spec Author:** Research & Specification Agent
**Date:** 2026-03-18
**Status:** Ready for Implementation

---

## 1. Current State

### 1.1 File Under Audit

`io.github.up.json` — the Flatpak application manifest.

### 1.2 Current `finish-args` Block (verbatim)

```json
"finish-args": [
    "--share=ipc",
    "--socket=fallback-x11",
    "--socket=wayland",
    "--talk-name=org.freedesktop.Flatpak",
    "--talk-name=org.freedesktop.PolicyKit1",
    "--filesystem=host:ro",
    "--talk-name=org.freedesktop.PackageKit"
]
```

The two entries being changed:

| Line | Permission | Finding |
|------|-----------|---------|
| `"--filesystem=host:ro"` | Read-only access to the **entire host filesystem** | Finding #2 |
| `"--talk-name=org.freedesktop.PackageKit"` | D-Bus session access to PackageKit daemon | Finding #3 |

---

## 2. Finding #2 Analysis — `--filesystem=host:ro`

### 2.1 What `--filesystem=host:ro` Grants

This permission exposes the entire host filesystem tree (all real paths under `/`) as read-only inside the Flatpak sandbox. This is an extremely coarse permission that gives the sandboxed application access to:
- All user home directory files
- All system configuration files
- All installed software paths
- Secrets, SSH keys, GPG keyrings, browser profiles, etc.

This violates the principle of least privilege for a sandboxed desktop application.

### 2.2 Complete Audit of Host Filesystem Paths Actually Read

Every specific filesystem path accessed at runtime was identified by reading all source files. Virtual filesystems (`/proc`, `/sys`) and the Flatpak-managed path (`/.flatpak-info`) are addressed separately below.

#### Table: Real Filesystem Paths Read by the Application

| Path | Source File | Line(s) | Access Type | Purpose |
|------|------------|---------|-------------|---------|
| `/etc/os-release` | `src/upgrade.rs` | 45 | `fs::read_to_string` | Parse distro ID, name, version for `detect_distro()` |
| `/etc/os-release` | `src/upgrade.rs` | 478 | `fs::read_to_string` | Re-read VERSION_ID in `check_fedora_upgrade()` fallback |
| `/etc/nixos` | `src/backends/nix.rs` | 10 | `Path::new(...).exists()` | Directory existence check for NixOS detection (`is_nixos()`) |
| `/etc/nixos/flake.nix` | `src/backends/nix.rs` | 15 | `Path::new(...).exists()` | File existence check for flake detection (`is_nixos_flake()`) |
| `/etc/nixos/flake.nix` | `src/upgrade.rs` | 29 | `Path::new(...).exists()` | Duplicate flake detection in `detect_nixos_config_type()` |
| `/etc/nixos/flake.lock` | `src/backends/nix.rs` | 180 | `tokio::fs::read_to_string` | Parse JSON lock file to count tracked flake inputs (`count_available()`) |
| `~/.nix-profile/manifest.json` | `src/backends/nix.rs` | 144–145 | `fs::read_to_string` | Read nix profile manifest to detect flake vs legacy profile format |

#### Paths Handled Separately (no `--filesystem=` entry needed)

| Path | Source File | Line | Reason No Permission Needed |
|------|------------|------|-----------------------------|
| `/proc/sys/kernel/hostname` | `src/backends/nix.rs` | 19 | `/proc` is a virtual filesystem automatically mounted in the Flatpak sandbox. `--filesystem=` entries control access to real host filesystem subtrees only. `/proc/sys/kernel/hostname` is readable by default within the sandbox without any explicit permission. |
| `/proc/sys/kernel/hostname` | `src/upgrade.rs` | 37 | Same as above — duplicate read in `detect_hostname()`. |
| `/.flatpak-info` | `src/reboot.rs` | 9 | This file is automatically injected into every Flatpak sandbox by the runtime itself. It is always accessible inside the sandbox and requires no `--filesystem=` permission. Used only for sandbox-presence detection in `reboot()`. |

### 2.3 Flatpak Sandbox Behaviour — `/proc`

Flatpak creates a new mount namespace for each sandboxed application. Within that namespace:
- `/proc` is mounted as a standard `procfs` for the sandbox's PID namespace
- `/proc/sys` (a subset of `sysctl` interface) is available and readable without special permissions
- `/proc/sys/kernel/hostname` reflects the host's UTS namespace hostname because Flatpak does **not** create a new UTS namespace by default (doing so would require `--unshare=uts`, which is not in this manifest)
- Therefore `/proc/sys/kernel/hostname` is correctly readable inside the sandbox without any `--filesystem=` entry

### 2.4 Minimal Replacement Permissions

The three `--filesystem=` entries below replace `--filesystem=host:ro` completely:

```json
"--filesystem=/etc/os-release:ro",
"--filesystem=/etc/nixos:ro",
"--filesystem=~/.nix-profile:ro"
```

#### Rationale for Each Entry

| Entry | Covers | Replaces |
|-------|--------|---------|
| `--filesystem=/etc/os-release:ro` | Single file. Covers both reads in `upgrade.rs` at lines 45 and 478. Required on every Linux system the app runs on. | `--filesystem=host:ro` (always active) |
| `--filesystem=/etc/nixos:ro` | Entire `/etc/nixos/` directory tree. Covers: `is_nixos()` (dir exists check), `is_nixos_flake()` (file exists check), `detect_nixos_config_type()` (file exists check), `count_available()` (reads `flake.lock`), and all NixOS upgrade command arguments passed to `pkexec`. Only accessed on NixOS; Flatpak silently grants permission even if the path does not exist on non-NixOS systems. | `--filesystem=host:ro` (NixOS-specific) |
| `--filesystem=~/.nix-profile:ro` | The user's active Nix profile directory under `$HOME`. Covers `manifest.json` read at `nix.rs` lines 144–145. Used only on non-NixOS systems with a Nix installation. `~` is expanded to `$HOME` by the Flatpak runtime at launch time. | `--filesystem=host:ro` (non-NixOS Nix-specific) |

### 2.5 Symlink Caveat for `~/.nix-profile`

On most Linux systems with Nix installed, `~/.nix-profile` is a symbolic link pointing to the actual profile directory under `/nix/var/nix/profiles/per-user/$USER/profile`. Flatpak's filesystem access grants follow symlinks only if the symlink target also falls within a permitted path. Since `/nix/` is not listed in the proposed permissions, reading through `~/.nix-profile/manifest.json` requires Flatpak to follow the symlink to `/nix/var/nix/...`, which would be denied.

**Recommended handling:**

1. **Preferred (narrower):** During implementation, add a `--filesystem=~/.nix-profile:ro` and test on a system with Nix installed. If Flatpak's sandbox follows the `~/.nix-profile` symlink (some versions do expose the symlink target in the bind-mount), this is sufficient.

2. **Fallback (if symlink is not followed):** Replace `--filesystem=~/.nix-profile:ro` with `--filesystem=/nix:ro`. This grants read-only access to the entire Nix store, which is much narrower than `--filesystem=host:ro` while still enabling Nix profile detection.

3. **Most targeted fallback:** Add both `--filesystem=~/.nix-profile:ro` and `--filesystem=/nix/var/nix/profiles:ro` — covers the symlink target without exposing the full Nix store.

The spec recommends **Option 1** as the implementation starting point with **Option 3** as the fallback if testing reveals the symlink is not followed.

### 2.6 Permissions That Are NOT Changing

These existing entries are correct and must remain:

| Permission | Purpose | Required? |
|-----------|---------|-----------|
| `--talk-name=org.freedesktop.Flatpak` | Required for `flatpak-spawn --host systemctl reboot` in `src/reboot.rs` | **Yes — keep** |
| `--talk-name=org.freedesktop.PolicyKit1` | Required for `pkexec` privilege escalation used by all OS update backends | **Yes — keep** |

---

## 3. Finding #3 Analysis — `org.freedesktop.PackageKit`

### 3.1 Search Results

**Search pattern:** `PackageKit|packagekit|package-kit` across all files in `src/`

**Result:** Zero matches in any source file.

Complete grep results by category:

| Search Term | Files Searched | Matches in `src/` |
|-------------|---------------|------------------|
| `PackageKit` | `src/**` | **0** |
| `org.freedesktop.PackageKit` | `src/**` | **0** |
| D-Bus session API calls (`dbus`, `DBus`, `gio::DBus`) | `src/**` | **0** |
| `packagekit` (lowercase) | `src/**` | **0** |

The only occurrence of `org.freedesktop.PackageKit` in the entire repository is in `io.github.up.json` itself (line 17 — the entry being audited). No D-Bus code exists anywhere in `src/`.

### 3.2 Architecture Confirmation

The application does not use D-Bus at all for package management. All package manager interactions (`apt`, `dnf`, `pacman`, `zypper`, `nix`, `flatpak`, `brew`) go through `CommandRunner` in `src/runner.rs`, which spawns child processes directly using `tokio::process::Command`. There is no `gio::DBus`, `dbus-glib`, or any other D-Bus client library in `Cargo.toml`.

### 3.3 Conclusion

The `--talk-name=org.freedesktop.PackageKit` permission is entirely unused. It grants the sandboxed application the ability to communicate with the system's PackageKit daemon (if installed), which is unnecessary and expands the application's D-Bus attack surface. It must be removed.

---

## 4. Proposed Changes

### 4.1 `io.github.up.json` — Modified `finish-args`

**Before:**

```json
"finish-args": [
    "--share=ipc",
    "--socket=fallback-x11",
    "--socket=wayland",
    "--talk-name=org.freedesktop.Flatpak",
    "--talk-name=org.freedesktop.PolicyKit1",
    "--filesystem=host:ro",
    "--talk-name=org.freedesktop.PackageKit"
]
```

**After:**

```json
"finish-args": [
    "--share=ipc",
    "--socket=fallback-x11",
    "--socket=wayland",
    "--talk-name=org.freedesktop.Flatpak",
    "--talk-name=org.freedesktop.PolicyKit1",
    "--filesystem=/etc/os-release:ro",
    "--filesystem=/etc/nixos:ro",
    "--filesystem=~/.nix-profile:ro"
]
```

**Change summary (line diff):**

| Action | Entry |
|--------|-------|
| REMOVE | `"--filesystem=host:ro"` |
| ADD | `"--filesystem=/etc/os-release:ro"` |
| ADD | `"--filesystem=/etc/nixos:ro"` |
| ADD | `"--filesystem=~/.nix-profile:ro"` |
| REMOVE | `"--talk-name=org.freedesktop.PackageKit"` |

### 4.2 No Source Code Changes Required

These changes are purely to the Flatpak manifest. No Rust source files need modification.

---

## 5. Risk Assessment

### 5.1 Regression Risk Matrix

| Scenario | Risk Level | Impact if Wrong | Mitigation |
|----------|-----------|----------------|------------|
| Distro detection breaks (`/etc/os-release` unreadable) | Negligible | App falls back to "unknown" distro, upgrade page still works but distro name shows "Unknown Linux" | `fs::read_to_string` uses `unwrap_or_default()` — graceful fallback already in place |
| NixOS detection breaks (`/etc/nixos` unreadable) | Low | On NixOS: `is_nixos()` returns false, NixOS backend treated as non-NixOS | `/etc/nixos` only present on NixOS; the permission grants access if path exists, is a no-op on other distros |
| `~/.nix-profile/manifest.json` unreadable (symlink not followed) | Low–Medium | Non-NixOS Nix users: `count_available()` silently falls back to `nix-env -u` path (already the fallback in code); `run_update()` is unaffected | Fallback in `nix.rs` lines 148–149 returns `false` on read failure, causing graceful degradation to legacy Nix path |
| `/.flatpak-info` unreadable | None | N/A — no `--filesystem=` entry was ever needed for this path | Sandbox auto-injects this file |
| `/proc/sys/kernel/hostname` unreadable | None | N/A — `/proc` is in the sandbox virtual filesystem, no permission change | Verified: Flatpak does not gate `/proc` access via `--filesystem=` |
| PackageKit removal breaks something | None | Zero D-Bus calls to PackageKit exist in source code | Confirmed by exhaustive source search |

### 5.2 Severity Justification

Both changes are MEDIUM severity fixes with LOW regression risk:
- **`--filesystem=host:ro`** exposed the user's entire home directory (including SSH keys, secrets, configs) to the sandboxed app read-only — an unnecessary overprivileging that violated Flatpak sandbox intent.
- **`org.freedesktop.PackageKit`** expanded D-Bus session surface needlessly; if a malicious dependency hijacked the app process, it could query or trigger PackageKit operations unintended by the app.

---

## 6. Verification Steps

### 6.1 Build Verification

```bash
# Verify the manifest is valid JSON
python3 -m json.tool io.github.up.json > /dev/null && echo "JSON valid"

# Build the Flatpak
flatpak-builder --user --install --force-clean builddir io.github.up.json
```

### 6.2 Functional Smoke Tests

Run each test on the matching platform/configuration:

#### Test 1 — Distro Detection (All Platforms)
```bash
# Inside the installed Flatpak
flatpak run io.github.up
# Expected: app launches, shows correct distro name in upgrade page header
# Failure indicator: "Unknown Linux" shown instead of correct distro
```

#### Test 2 — `/proc/sys/kernel/hostname` readable (NixOS)
```bash
flatpak run --command=bash io.github.up
cat /proc/sys/kernel/hostname
# Expected: prints the system hostname
# Failure indicator: "cat: /proc/sys/kernel/hostname: No such file or directory"
```

#### Test 3 — NixOS Detection (NixOS only)
```bash
flatpak run io.github.up
# Expected: Nix backend appears in the backend list; upgrade page shows NixOS config type
# Failure indicator: Nix backend missing or shows non-NixOS fallback on a NixOS system
```

#### Test 4 — Nix Profile Detection (non-NixOS with Nix installed)
```bash
flatpak run io.github.up
# Expected: Nix backend detected; update count shows correctly
# If symlink issue: update count shows 0 or error (graceful — not a crash)
# Failure indicator: crash or panic in Nix backend
```

#### Test 5 — Reboot Function (Flatpak)
```bash
# After applying updates
# Click "Reboot Now" button
# Expected: system reboot initiated via flatpak-spawn --host systemctl reboot
# Not directly testable without rebooting; verify logically via code review of reboot.rs
```

#### Test 6 — PackageKit Not Contacted
```bash
# Monitor D-Bus session bus during app run
dbus-monitor --session "sender='io.github.up'" 2>&1 | grep -i packagekit
# Expected: no output (no PackageKit D-Bus messages)
```

#### Test 7 — Manifest Permission Audit
```bash
# After installing the updated Flatpak
flatpak info --show-permissions io.github.up
# Expected output should contain:
#   filesystem=/etc/os-release:ro
#   filesystem=/etc/nixos:ro
#   filesystem=~/.nix-profile:ro
# Expected output should NOT contain:
#   filesystem=host:ro
#   talk-name=org.freedesktop.PackageKit
```

### 6.3 Regression Test — Desktop File Validation
```bash
# Ensure manifest changes don't break existing preflight
bash scripts/preflight.sh
# Expected: exit code 0, all steps pass
```

---

## 7. Implementation Instructions

The implementing agent must make exactly the following change to `io.github.up.json`:

1. Locate the `"finish-args"` array.
2. Remove the line: `"--filesystem=host:ro"`
3. Remove the line: `"--talk-name=org.freedesktop.PackageKit"`
4. Add these three lines in place of `"--filesystem=host:ro"`:
   ```json
   "--filesystem=/etc/os-release:ro",
   "--filesystem=/etc/nixos:ro",
   "--filesystem=~/.nix-profile:ro"
   ```
5. Preserve all other `finish-args` entries unchanged.
6. Verify the file is valid JSON after editing.

**No changes to any file in `src/` are required.**

---

## 8. Sources Consulted

1. **Flatpak Documentation — Sandbox Permissions**
   Flatpak's official documentation on `finish-args`, filesystem access rules, and named D-Bus permissions.
   Source: https://docs.flatpak.org/en/latest/sandbox-permissions.html

2. **Flatpak Documentation — Filesystem Access**
   Specific documentation on `--filesystem=` permission syntax, path granularity, and `host` vs specific path entries.
   Source: https://docs.flatpak.org/en/latest/sandbox-permissions-reference.html

3. **GNOME Flatpak Best Practices**
   GNOME developer documentation recommending specific path grants over `--filesystem=host`.
   Source: https://developer.gnome.org/documentation/tutorials/packaging/flatpak.html

4. **Freedesktop PackageKit D-Bus API**
   Documentation confirming `org.freedesktop.PackageKit` is a D-Bus interface, not a command-line tool, and is only accessible via D-Bus calls.
   Source: https://packagekit.freedesktop.org/

5. **Flatpak Issue Tracker / Community: `/proc` access in Flatpak sandboxes**
   Community consensus and Flatpak source code confirming `/proc` is mounted inside sandbox namespaces independently of `--filesystem=` permissions, and `/proc/sys/kernel/hostname` is readable by default.
   Source: Flatpak GitHub discussions and sandbox implementation in `common/flatpak-run.c`

6. **Linux Namespaces — UTS Namespace**
   Linux namespaces documentation confirming that hostname visibility depends on UTS namespace isolation; Flatpak does not create a new UTS namespace by default, making the host hostname visible inside the sandbox via `/proc/sys/kernel/hostname`.
   Source: https://man7.org/linux/man-pages/man7/namespaces.7.html

7. **Flatpak `/.flatpak-info` specification**
   Flatpak documentation confirming `/.flatpak-info` is automatically injected into every Flatpak sandbox and does not require any `--filesystem=` permission.
   Source: https://docs.flatpak.org/en/latest/flatpak-command-reference.html#flatpak-info

8. **OWASP Principle of Least Privilege**
   Security principle confirming that granting minimum required permissions reduces attack surface even in read-only contexts (path enumeration, secrets exfiltration).
   Source: https://owasp.org/www-community/Access_Control
