# Security Regression Fixes — Phase 1 Specification

**Date:** 2026-03-18  
**Project:** Up (GTK4/libadwaita system updater)  
**Scope:** Three confirmed functional regressions introduced by prior security hardening

---

## Executive Summary

A prior security fix hardened shell-injection attack surfaces in `src/backends/nix.rs` and
tightened Flatpak sandbox permissions in `io.github.up.json`. Both changes were correct in
intent but introduced three regressions:

1. **`validate_hostname()` rejects dots** — NixOS users with dotted hostnames (FQDNs such
   as `myhost.local`, `nixos.lan`, `host.example.com`) receive a runtime error instead of
   a successful update, because `.` was excluded from the allowlist.

2. **Flatpak sandbox: all backends invisible** — Replacing `--filesystem=host:ro` with three
   narrow path mounts removed access to host binary directories. Every backend detection call
   (`which::which("apt")`, etc.) now fails inside the sandbox, so the app presents no backends.

3. **`nix flake update /etc/nixos` broken on Nix ≥ 2.19** — The positional argument form of
   `nix flake update <path>` was deprecated; on NixOS 24.05+ the argument is interpreted as
   an input name, not a flake path. The flake lock file is never updated.

All three regressions are one- to three-line fixes. None reintroduces the original
injection vulnerability.

---

## Regression #1 — `validate_hostname()` rejects dots in FQDNs

### Current Broken Code

**File:** `src/backends/nix.rs`, lines 37–42

```rust
if !hostname
    .chars()
    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
{
    return Err(format!("Invalid hostname: {:?}", hostname));
}
```

The allowed character set is: `[A-Za-z0-9\-_]`. The `.` (dot) character is absent.

### Why It Is Broken

Any NixOS installation with a dotted hostname — including the very common `<name>.local`
mDNS-style names and any FQDN — will fail at runtime with the error:

```
Invalid hostname: "myhost.local"
```

The hostname is read from `/proc/sys/kernel/hostname` and used to construct the flake
output selector string:

```rust
let flake_arg = format!("/etc/nixos#{}", hostname);
```

This string is then passed as a single element in an argv array to `runner.run()`:

```rust
runner.run("pkexec", &[
    ...,
    "nixos-rebuild", "switch",
    "--flake", &flake_arg,
]).await
```

**Zero shell evaluation occurs.** The value is passed directly to the kernel `execve` syscall
as a plain argv token. A dot inside an argv element carries no injection risk whatsoever.

The original injection risk was from `sh -c` interpolation of the hostname. That `sh -c`
usage was correctly removed in the prior security fix. The hostname restriction to non-dot
characters was an overshoot.

### RFC 1123 / DNS Compliance

RFC 1123 §2.1 specifies valid hostname characters as: `[A-Za-z0-9\-]` per label, with dots
as label separators. The underscore (`_`) is already a non-standard extension in the current
allowlist. Adding `.` makes the allowlist a strict superset of RFC 1123-valid hostnames.

A FQDN may be up to 253 characters total (RFC 1035 §2.3.4). The current hard limit of
63 characters — which is the per-label maximum, not the FQDN maximum — will reject valid
FQDNs such as `myhost.example.com` (21 chars, fine) but also any FQDN longer than 63 chars.
The length check should be updated to 253 to match the FQDN maximum.

### Exact Fix

**In `src/backends/nix.rs`**, make two changes to `validate_hostname()`:

**Change 1** — Update the length limit from per-label (63) to FQDN maximum (253):

```rust
// BEFORE:
if hostname.len() > 63 {
    return Err(format!(
        "hostname is too long ({} chars, max 63)",
        hostname.len()
    ));
}

// AFTER:
if hostname.len() > 253 {
    return Err(format!(
        "hostname is too long ({} chars, max 253)",
        hostname.len()
    ));
}
```

**Change 2** — Add `.` to the allowed character set:

```rust
// BEFORE:
if !hostname
    .chars()
    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')

// AFTER:
if !hostname
    .chars()
    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
```

### Security Neutrality

The prior vulnerability was `sh -c "nixos-rebuild switch --flake /etc/nixos#$HOSTNAME"`:
a shell-expanded variable that could be injected with `;`, `` ` ``, `$()`, `|`, etc.

The current code uses `runner.run("pkexec", &[..., &flake_arg])` — a direct `execve` with
no shell involvement. There is no mechanism by which any character in `hostname`, including
`.`, can escape the argv element boundary. Adding `.` to the allowlist does not reintroduce
the original vulnerability.

### Files Affected

- `src/backends/nix.rs`

---

## Regression #2 — Flatpak sandbox: all backends invisible

### Current Broken State

**File:** `io.github.up.json`, `finish-args` array (full current contents):

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

The entries `--filesystem=/etc/os-release:ro`, `--filesystem=/etc/nixos:ro`, and
`--filesystem=~/.nix-profile:ro` replaced the original `--filesystem=host:ro`. The host
binary directories (`/usr/bin`, `/usr/local/bin`, etc.) are no longer mounted in the sandbox.

### Why It Is Broken

Every backend detection function calls `which::which()`, which searches `$PATH`:

| Backend | Detection call | Binary location on host |
|---------|---------------|------------------------|
| APT | `which::which("apt")` | `/usr/bin/apt` |
| DNF | `which::which("dnf")` | `/usr/bin/dnf` |
| Pacman | `which::which("pacman")` | `/usr/bin/pacman` |
| Zypper | `which::which("zypper")` | `/usr/bin/zypper` |
| Flatpak | `which::which("flatpak")` | `/usr/bin/flatpak` |
| Homebrew | `which::which("brew")` | `/usr/local/bin/brew` or `/home/linuxbrew/.linuxbrew/bin/brew` |
| Nix | `which::which("nix")` | `~/.nix-profile/bin/nix` |

**Source references** (from `src/backends/`):
- `os_package_manager.rs` lines 6–16: four sequential `which::which()` calls, first match wins
- `flatpak.rs` line 4: `which::which("flatpak").is_ok()`
- `homebrew.rs` line 4: `which::which("brew").is_ok()`
- `nix.rs` line 4: `which::which("nix").is_ok()`

There is **no Flatpak-aware fallback detection path** in any of these modules. No backend
checks `/run/host/usr/bin/`, uses `flatpak-spawn --host which <cmd>`, or reads host paths
from any alternative location.

Inside the Flatpak sandbox, `$PATH` resolves only against the sandbox filesystem (provided by
the GNOME runtime). Without a bind-mount making host `/usr/bin` accessible, every
`which()` call returns `Err(which::Error::CannotFindBinaryPath)`, all backends are
undetected, and `detect_backends()` returns an empty `Vec`.

### How Flatpak Filesystem Permissions Work

Flatpak's `--filesystem=/path:ro` bind-mounts the **host** path at the **same path** inside
the sandbox. This means:
- `--filesystem=/usr:ro` mounts the host's `/usr` tree at `/usr` inside the sandbox,
  making `/usr/bin/apt`, `/usr/bin/dnf`, etc. accessible at their canonical paths.
- `$PATH` inside the sandbox already contains `/usr/bin`; no PATH changes are needed.
- `which::which("apt")` then succeeds exactly as it would outside the sandbox.

`--filesystem=/usr:ro` is a strict subtree, covering `/usr/bin`, `/usr/local/bin`,
`/usr/sbin`, and all other subdirectories of `/usr` in one entry. A separate
`--filesystem=/usr/local:ro` entry is therefore **redundant** and not needed.

### Option Comparison

| Option | Paths granted | Covers all standard distro package managers | Security cost vs. original |
|--------|--------------|---------------------------------------------|---------------------------|
| A — `--filesystem=/usr:ro` | `/usr/**` | Yes (`apt`, `dnf`, `pacman`, `zypper`, `flatpak`) | Far less permissive than `host:ro`; does NOT expose `/home`, `/var`, `/etc/shadow`, `/root` |
| B — `/usr:ro` + `/usr/local:ro` | `/usr/**` (redundant second entry) | Same as A | Same as A, but with a redundant entry |
| C — `--filesystem=host:ro` (full revert) | Entire host filesystem | Yes | Defeats the security fix; not acceptable |

**Recommendation: Option A** — add exactly one entry: `"--filesystem=/usr:ro"`.

### Additional Path Analysis

| Tool | Typical path | Covered by `/usr:ro`? |
|------|-------------|----------------------|
| apt, dnf, pacman, zypper, flatpak | `/usr/bin/<name>` | ✅ Yes |
| brew (Linuxbrew) | `/home/linuxbrew/.linuxbrew/bin/brew` | ❌ No — outside `/usr` |
| nix | `~/.nix-profile/bin/nix` | Already covered by `~/.nix-profile:ro` |

Homebrew on Linux installs to `/home/linuxbrew/.linuxbrew/bin/brew` (multi-user) or
`~/.linuxbrew/bin/brew` (single-user). Neither is under `/usr`. However, Homebrew is
explicitly a cross-platform tool and its Linux usage as a system package manager inside a
Flatpak sandbox is a niche edge case. The primary regression (OS package managers and
Flatpak itself being invisible) is fully resolved by `--filesystem=/usr:ro`.

If Homebrew support inside Flatpak is a future requirement, a separate spec should address it
with a targeted `--filesystem` entry or a `flatpak-spawn --host` detection probe.

### Exact Fix

**File:** `io.github.up.json`

Add `"--filesystem=/usr:ro"` to the `finish-args` array, immediately before the existing
`/etc/os-release` entry for logical grouping:

```json
// BEFORE:
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

// AFTER:
"finish-args": [
    "--share=ipc",
    "--socket=fallback-x11",
    "--socket=wayland",
    "--talk-name=org.freedesktop.Flatpak",
    "--talk-name=org.freedesktop.PolicyKit1",
    "--filesystem=/usr:ro",
    "--filesystem=/etc/os-release:ro",
    "--filesystem=/etc/nixos:ro",
    "--filesystem=~/.nix-profile:ro"
]
```

No Rust source code changes are required.

### Security Neutrality

The original `--filesystem=host:ro` granted read access to the entire host filesystem,
including `/home`, `/var`, `/root`, `/etc/shadow`, and `/proc`. The new
`--filesystem=/usr:ro` grants read access only to the `/usr` subtree, which contains
installed programs and shared libraries — no user data, no credentials, no sensitive
configuration. This is aligned with the principle of least privilege: the app needs to
*detect* host binaries, not read arbitrary host files.

The security improvement from the prior fix (removing `host:ro`) is substantially preserved.
The new permission surface is `/usr:ro` instead of `host:ro`.

### Files Affected

- `io.github.up.json`

---

## Regression #3 — `nix flake update /etc/nixos` broken on Nix ≥ 2.19

### Current Broken Code

**File:** `src/backends/nix.rs`, the first `runner.run()` call in the flake-based NixOS branch
(lines approximately 84–96):

```rust
runner.run(
    "pkexec",
    &[
        "env",
        "PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin",
        "nix",
        "--extra-experimental-features",
        "nix-command flakes",
        "flake",
        "update",
        "/etc/nixos",          // ← broken: treated as input name on Nix ≥ 2.19
    ],
).await
```

### Why It Is Broken

The `nix flake update` subcommand changed its argument semantics between Nix versions:

| Nix version | `nix flake update /etc/nixos` means |
|-------------|-------------------------------------|
| < 2.19 | Update the flake at path `/etc/nixos` |
| ≥ 2.19 (NixOS 24.05+) | Update the flake **input** named `/etc/nixos` (no such input → silent failure or error) |

NixOS 24.05 ships Nix 2.21, and NixOS 24.11 / 25.05 ship later versions. The majority of
active NixOS installations are already on Nix ≥ 2.19. On these systems, the `nix flake update`
call above either fails with a "flake input not found" error or silently does nothing, leaving
the `flake.lock` file unchanged. The subsequent `nixos-rebuild switch --flake ...` then
rebuilds from the stale lock file, which makes the update appear to succeed while not actually
fetching any new inputs.

### Correct API (Nix ≥ 2.19)

The `--flake <path>` flag specifies the flake path and has been available since before 2.19:

```
nix flake update --flake /etc/nixos
```

This form is **backward-compatible**: it works on Nix < 2.19 as well, because `--flake` was
already accepted as an option in earlier versions. Using `--flake` is now the canonical form
recommended in the Nix 2.19 release notes (https://nixos.org/manual/nix/stable/release-notes/rl-2.19).

### Exact Fix

**In `src/backends/nix.rs`**, insert `"--flake"` before `"/etc/nixos"` in the args array:

```rust
// BEFORE:
runner.run(
    "pkexec",
    &[
        "env",
        "PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin",
        "nix",
        "--extra-experimental-features",
        "nix-command flakes",
        "flake",
        "update",
        "/etc/nixos",
    ],
).await

// AFTER:
runner.run(
    "pkexec",
    &[
        "env",
        "PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin",
        "nix",
        "--extra-experimental-features",
        "nix-command flakes",
        "flake",
        "update",
        "--flake",
        "/etc/nixos",
    ],
).await
```

The only change is inserting `"--flake"` as a new array element before `"/etc/nixos"`.

### Backward Compatibility with Nix < 2.19

The `--flake` flag accepts a path argument and was supported in Nix's `nix flake update`
command long before 2.19. All commonly deployed Nix versions that support flakes
(`nix-command flakes` experimental features, meaning Nix ≥ 2.4) accept `--flake <path>`
as the canonical way to specify the target flake directory. This fix works correctly across
the full range of Nix versions that support flake-based NixOS configurations.

### Security Neutrality

No shell evaluation is involved. The value `"/etc/nixos"` is a hardcoded string literal in
the source code, not user-supplied input. Inserting `"--flake"` before it changes argv
structure but introduces no new attack surface.

### Files Affected

- `src/backends/nix.rs`

---

## Implementation Summary

### Files to Modify

| File | Change |
|------|--------|
| `src/backends/nix.rs` | (1) Update `validate_hostname()` length limit from 63 → 253; add `.` to allowed charset. (3) Insert `"--flake"` before `"/etc/nixos"` in the `nix flake update` args array. |
| `io.github.up.json` | (2) Add `"--filesystem=/usr:ro"` to `finish-args`. |

### No New Dependencies

All three fixes modify existing logic or configuration values. No new Cargo crates,
Flatpak permissions beyond `/usr:ro`, or external tooling are required.

### Post-Fix Verification

After implementation, the review subagent should verify:

1. `cargo build` compiles without errors
2. `cargo clippy -- -D warnings` produces no warnings
3. A hostname like `myhost.local` passes `validate_hostname()` without error
4. The `io.github.up.json` diff adds exactly one line (`"--filesystem=/usr:ro"`)
5. The `nix flake update` args array contains `"--flake"` immediately before `"/etc/nixos"`
