# Specification: Auto-Source Version from `Cargo.toml`

**Backlog Item 11** — Eliminate hand-sync of `version = "1.0.3"` across `Cargo.toml`, `meson.build`, and `flake.nix`.

---

## 1. Current State

| File | Location | Hard-coded Value |
|------|----------|-----------------|
| `Cargo.toml` | Line 3: `version = "1.0.3"` | `1.0.3` — **source of truth** |
| `meson.build` | Line 2: `version: '1.0.3'` inside `project()` | `1.0.3` — must derive from Cargo.toml |
| `flake.nix` | Line 17: `version = "1.0.3";` inside `buildRustPackage` | `1.0.3` — must derive from Cargo.toml |

`build.rs` only compiles GLib resources (`glib_build_tools::compile_resources`) — no version logic and no changes needed. The Rust binary already receives the correct version at compile time via `CARGO_PKG_VERSION`, which Cargo sets directly from `Cargo.toml`.

---

## 2. Problem

Every release requires updating the version in three separate files. Any missed edit causes the installed binary, the Meson build system, and the Nix derivation to report mismatched versions. There is no automated enforcement, so drift is inevitable.

---

## 3. Proposed Changes

### 3.1 `meson.build` — inline `run_command()` in `project()`

**Constraint:** Meson requires `project()` to be the first statement. Variables cannot be declared before it, and `import()` (including `fs`) cannot be called before it. Therefore the only way to derive the version dynamically inside `project()` is via an inline `run_command()` call as a keyword-argument expression.

**Approach:** Use `grep` to extract the first `version = "…"` line from `Cargo.toml`, then use Meson's built-in string `.split()` to isolate the bare version number. `grep` is universally available on all Linux systems this app targets.

**Before:**
```meson
project('up',
  version: '1.0.3',
  license: 'GPL-3.0-or-later',
)
```

**After:**
```meson
project('up',
  version: run_command(
    'grep', '-m', '1', '^version', 'Cargo.toml',
    check: true,
  ).stdout().strip().split('"')[1],
  license: 'GPL-3.0-or-later',
)
```

**How it works:**
- `grep -m 1 '^version' Cargo.toml` → `version = "1.0.3"`
- `.stdout().strip()` → `version = "1.0.3"` (whitespace trimmed)
- `.split('"')[1]` → `1.0.3`

`check: true` causes Meson to abort configuration with an error if `grep` fails, rather than silently producing an empty version string.

The rest of `meson.build` is **unchanged** — `fs = import('fs')` and all subsequent lines remain exactly as-is.

---

### 3.2 `flake.nix` — `builtins.fromTOML`

**Constraint:** The flake pins `nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05"`. `builtins.fromTOML` was introduced in Nix 2.6.0 (released November 2022, bundled with NixOS 22.05). nixos-25.05 is far beyond that threshold — `builtins.fromTOML` is fully available.

**Approach:** Replace the literal string with a Nix expression that reads and parses `./Cargo.toml` at evaluation time.

**Before:**
```nix
packages.default = pkgs.rustPlatform.buildRustPackage {
  pname = "up";
  version = "1.0.3";
  src = ./.;
```

**After:**
```nix
packages.default = pkgs.rustPlatform.buildRustPackage {
  pname = "up";
  version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
  src = ./.;
```

**How it works:**
- `builtins.readFile ./Cargo.toml` — reads the file as a string at Nix evaluation time
- `builtins.fromTOML (…)` — parses it into a Nix attribute set
- `.package.version` — accesses the `[package]` table's `version` key → `"1.0.3"`

No other lines in `flake.nix` change.

---

### 3.3 `build.rs` — no change

`build.rs` calls `glib_build_tools::compile_resources(...)` only. Rust already sources the version from `Cargo.toml` via the `CARGO_PKG_VERSION` environment variable at compile time. No modifications required.

---

### 3.4 `scripts/preflight.sh` — no change

The preflight script runs `cargo fmt`, `cargo clippy`, `cargo build`, `cargo test`, `desktop-file-validate`, and `appstreamcli validate`. None of these are version-sensitive. No modifications required.

---

## 4. Implementation Steps

1. Open `meson.build`.
2. Replace the two-line `version: '1.0.3',` literal inside `project()` with the `run_command(...)` expression shown in §3.1.
3. Open `flake.nix`.
4. Replace the single line `version = "1.0.3";` inside `buildRustPackage` with the `builtins.fromTOML` expression shown in §3.2.
5. Do **not** touch `Cargo.toml`, `build.rs`, or `scripts/preflight.sh`.

Total modified files: **2** (`meson.build`, `flake.nix`).

---

## 5. Risks and Mitigations

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| `grep` not available at Meson configure time | Very low — `grep` is a POSIX mandatory utility on all Linux distros | `check: true` aborts configure with a clear error rather than silently producing a bad version; the error message will identify the missing tool |
| Meson `.split('"')[1]` fails if TOML format changes | Very low — the `version = "x.y.z"` line in `[package]` is required by Cargo and will always match | If Cargo.toml were ever restructured (e.g., version moved to workspace), the grep pattern `^version` would still match the first occurrence |
| `builtins.fromTOML` not available in Nix | Not applicable — flake pins nixos-25.05, which ships Nix ≥ 2.18; `fromTOML` was added in 2.6 | None needed |
| `builtins.readFile` path resolution | Low — `./Cargo.toml` resolves relative to the flake's source root, which is always the repository root | Standard Nix flake convention; used by countless projects |
| `run_command()` before source tree is present (e.g., dist tarballs without Cargo.toml) | Very low — the `meson.build` already references `Cargo.toml` in the `cargo_build` custom target | `check: true` will produce a clear configure-time error if the file is absent |

---

## 6. Verification

After implementation, verify with:

```bash
# Meson: confirm version is derived correctly
meson setup builddir
grep 'up' builddir/meson-info/intro-projectinfo.json
# Expected: "version": "1.0.3"

# Nix: confirm version in derivation
nix eval .#packages.x86_64-linux.default.version
# Expected: "1.0.3"

# Bump test: change version in Cargo.toml only, re-run both —
# both should report the new version without any other edits.
```

---

## 7. Files to Modify

| File | Change |
|------|--------|
| `meson.build` | Replace `version: '1.0.3'` literal with `run_command(...)` expression |
| `flake.nix` | Replace `version = "1.0.3";` literal with `builtins.fromTOML` expression |
