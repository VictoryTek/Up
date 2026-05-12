# Spec: Nix Build Fix — `up-daemon` Not Found During `installPhase`

**Feature name:** `nix_daemon_build`  
**Date:** 2026-05-12  
**Author:** Research Subagent  
**Status:** Ready for Implementation

---

## 1. Current State Analysis

### 1.1 `flake.nix` Derivation (relevant excerpt)

```nix
packages.default = pkgs.rustPlatform.buildRustPackage {
  pname = "up";
  version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
  src = ./.;

  cargoLock = {
    lockFile = ./Cargo.lock;
  };

  # ... nativeBuildInputs, buildInputs, preFixup ...

  postInstall = ''
    install -Dm644 data/io.github.up.desktop \
      $out/share/applications/io.github.up.desktop
    # ... other data files ...

    # D-Bus daemon  ← THIS LINE FAILS
    install -Dm755 target/release/up-daemon $out/libexec/up-daemon

    install -Dm644 data/io.github.up.Daemon.service \
      $out/lib/systemd/system/io.github.up.Daemon.service
    # ...
  '';
};
```

**There is no `cargoBuildFlags` attribute set.** `buildRustPackage` therefore invokes `cargoBuildHook` with no extra cargo flags, building only the root crate.

### 1.2 `Cargo.toml` Workspace Declaration

```toml
[workspace]
members = [".", "daemon"]

[package]
name = "up"
version = "2.0.0"
edition = "2021"
```

The workspace has two members:
- `.` — the root `up` crate (produces binary `up`)
- `daemon` — the `daemon/` sub-crate (produces binary `up-daemon`)

### 1.3 `daemon/Cargo.toml` — Binary Name Confirmation

```toml
[package]
name = "up-daemon"
version = "2.0.0"
edition = "2021"

[[bin]]
name = "up-daemon"
path = "src/main.rs"
```

The daemon package explicitly declares its binary as `up-daemon`.

---

## 2. Problem Definition

### 2.1 Primary Bug: `up-daemon` Not Compiled

`buildRustPackage` invokes `cargoBuildHook` under the hood. Inspecting the nixpkgs hook source (`pkgs/build-support/rust/hooks/cargo-build-hook.sh`):

```bash
cargoBuildHook() {
    local flagsArray=(
        "-j" "$NIX_BUILD_CORES"
        "--target" "@rustcTargetSpec@"   # always set to host/cross target triple
        "--offline"
    )
    # profile, features flags ...
    concatTo flagsArray cargoBuildFlags  # ← only appended if cargoBuildFlags is set
    cargo build "${flagsArray[@]}"
}
```

Without `cargoBuildFlags = [ "--workspace" ]`, cargo receives no `--workspace` flag and builds **only the default members of the root manifest** — in practice, just the `up` binary. The `daemon/` workspace member is never compiled, so `up-daemon` does not exist anywhere in `target/`.

### 2.2 Secondary Bug: Wrong Path in `postInstall`

The nixpkgs `cargoBuildHook` **always** invokes cargo with `--target @rustcTargetSpec@` (the host Rust target triple, e.g. `x86_64-unknown-linux-gnu`). When cargo receives an explicit `--target` flag, it writes artifacts to the **architecture-specific subdirectory**:

```
target/<arch-triple>/release/    ← actual output location in nixpkgs
target/release/                  ← does NOT exist; unused when --target is set
```

This is documented explicitly in the nixpkgs Rust manual:
> "Those tests are likely to fail because we use `cargo --target` during the build. This means that the artifacts are stored in `target/<architecture>/release/`, rather than in `target/release/`."

The `cargoInstallHook` (`pkgs/build-support/rust/hooks/cargo-install-hook.sh`) uses `target/@targetSubdirectory@/$cargoBuildType` (the arch-specific path) to locate and install binaries to `$out/bin/`. After it runs, both `up` and `up-daemon` (when compiled) are placed in `$out/bin/`.

The current `postInstall` line:
```bash
install -Dm755 target/release/up-daemon $out/libexec/up-daemon
```
is wrong for **two independent reasons**:
1. `up-daemon` was never compiled (primary bug)
2. Even if compiled, `target/release/` is not the path nixpkgs uses — the arch-specific subdir is used

### 2.3 Why Tests Still Pass

All 99 tests reside in the root `up` crate. Inspecting `daemon/src/` confirms there are **no `#[test]` or `#[cfg(test)]` blocks** in the daemon source. Because `cargo test` (without `--workspace`) only tests the root package, all 99 tests pass independently of whether `up-daemon` is ever compiled.

---

## 3. Proposed Solution

### 3.1 Fix 1: Add `cargoBuildFlags = [ "--workspace" ]`

Add the following attribute to the `buildRustPackage` derivation in `flake.nix`:

```nix
cargoBuildFlags = [ "--workspace" ];
```

**Placement:** Inside `pkgs.rustPlatform.buildRustPackage { ... }`, alongside the existing `cargoLock`, `nativeBuildInputs`, etc.

**Effect:** `cargoBuildHook` will invoke:
```
cargo build --target x86_64-unknown-linux-gnu --offline --profile release --workspace
```
This compiles all workspace members — both `up` and `up-daemon` — placing binaries at:
```
target/x86_64-unknown-linux-gnu/release/up
target/x86_64-unknown-linux-gnu/release/up-daemon
```

`cargoInstallHook` then auto-installs all executables found in that directory to `$out/bin/`:
```
$out/bin/up
$out/bin/up-daemon
```

### 3.2 Fix 2: Correct the `postInstall` Path for `up-daemon`

Replace the broken line in `postInstall`:

```bash
# BEFORE (broken — wrong path, arch-specific subdir not used):
install -Dm755 target/release/up-daemon $out/libexec/up-daemon
```

```bash
# AFTER (correct — binary is already installed by cargoInstallHook):
mkdir -p $out/libexec
mv $out/bin/up-daemon $out/libexec/up-daemon
```

**Rationale:** By the time `postInstall` executes (it is called via `runHook postInstall` inside `cargoInstallHook`), `cargoInstallHook` has already copied all workspace binaries from `target/<arch>/release/` to `$out/bin/`. The `up-daemon` binary is a system daemon — it should live in `$out/libexec/`, not `$out/bin/`. Using `mv` removes it from `$out/bin/` and places it correctly in `$out/libexec/`.

### 3.3 Complete Diff — `flake.nix`

```nix
packages.default = pkgs.rustPlatform.buildRustPackage {
  pname = "up";
  version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
  src = ./.;

  cargoLock = {
    lockFile = ./Cargo.lock;
  };

+  cargoBuildFlags = [ "--workspace" ];

  nativeBuildInputs = with pkgs; [
    pkg-config
    wrapGAppsHook4
    glib
    gtk4
  ];

  buildInputs = with pkgs; [
    gtk4
    libadwaita
    dbus
    hicolor-icon-theme
  ];

  preFixup = ''
    gappsWrapperArgs+=(--prefix XDG_DATA_DIRS : "$out/share")
  '';

  postInstall = ''
    install -Dm644 data/io.github.up.desktop \
      $out/share/applications/io.github.up.desktop
    install -Dm644 data/io.github.up.metainfo.xml \
      $out/share/metainfo/io.github.up.metainfo.xml
    install -Dm644 data/io.github.up.policy \
      $out/share/polkit-1/actions/io.github.up.policy
    install -Dm644 data/icons/hicolor/256x256/apps/io.github.up.png \
      $out/share/icons/hicolor/256x256/apps/io.github.up.png
    gtk4-update-icon-cache -qtf $out/share/icons/hicolor

    # D-Bus daemon
-   install -Dm755 target/release/up-daemon $out/libexec/up-daemon
+   mkdir -p $out/libexec
+   mv $out/bin/up-daemon $out/libexec/up-daemon
    install -Dm644 data/io.github.up.Daemon.service \
      $out/lib/systemd/system/io.github.up.Daemon.service
    install -Dm644 data/io.github.up.Daemon.conf \
      $out/share/dbus-1/system.d/io.github.up.Daemon.conf

    # Plugin backend descriptors
    install -Dm644 data/backends.d/apk.yaml \
      $out/share/up/backends.d/apk.yaml
    install -Dm644 data/backends.d/xbps.yaml \
      $out/share/up/backends.d/xbps.yaml
  '';

  meta = with pkgs.lib; {
    description = "A modern Linux system update & upgrade app";
    homepage = "https://github.com/user/up";
    license = licenses.gpl3Plus;
    platforms = platforms.linux;
    mainProgram = "up";
  };
};
```

---

## 4. `cargoTestFlags` Assessment

**Recommendation: Do NOT add `--workspace` to `cargoTestFlags`.**

| Factor | Detail |
|--------|--------|
| Daemon tests | No `#[test]` or `#[cfg(test)]` blocks exist anywhere in `daemon/src/` |
| Root package tests | 99 tests, all passing, cover main application logic |
| Risk of `--workspace` in tests | Would add an extra (empty) test pass for the daemon crate — harmless but wasteful |
| Correct scope | `cargo test` on the root package alone is sufficient and correct |

The default `checkPhase` (no `cargoTestFlags` set) runs `cargo test` scoped to the root package. This is the correct and intended behaviour. No change is needed.

---

## 5. Other Derivation Attributes — Assessment

| Attribute | Change Required? | Notes |
|-----------|-----------------|-------|
| `cargoLock` | **No** | `./Cargo.lock` covers all workspace members; no change needed |
| `src` | **No** | `src = ./.` includes the entire workspace root, including `daemon/` |
| `nativeBuildInputs` | **No** | GTK/pkg-config tools are needed only for the `up` GUI binary |
| `buildInputs` | **No** | `up-daemon` only links against `zbus`, `tokio`, `libc` — all pure Rust crates in Cargo.lock |
| `meta.mainProgram` | **No** | `"up"` remains correct; `up-daemon` is not the primary user-facing program |
| `doCheck` | **No** | Tests pass correctly without `--workspace` |

---

## 6. Risks and Mitigations

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| Build time increase | Low | Daemon is small (~10 source files, no heavy deps beyond `zbus`/`tokio`) |
| `cargoInstallHook` installs `up-daemon` to `$out/bin/` before `mv` | Certain (by design) | The `mv` in Fix 2 removes it from `$out/bin/`; this is the intended behaviour |
| `cargoLock` hash mismatch | None | `Cargo.lock` is already present and already used; no new dependencies are added |
| `--workspace` changes which crates are tested | None | `cargoTestFlags` is left unchanged; tests still run only for the root package |
| `up-daemon` visible as a user binary | Eliminated | The `mv` from `$out/bin/` to `$out/libexec/` prevents accidental execution by users |
| Cross-compilation breakage | None | `cargoBuildFlags = ["--workspace"]` is safe for cross-compilation; `--target` is still set by `cargoBuildHook` |

---

## 7. No New External Dependencies

Both fixes are purely `flake.nix` attribute changes. No new crates, system libraries, or nixpkgs packages are introduced.

---

## 8. Implementation Steps

1. Open `flake.nix`
2. Inside `pkgs.rustPlatform.buildRustPackage { ... }`, add after `cargoLock`:
   ```nix
   cargoBuildFlags = [ "--workspace" ];
   ```
3. In `postInstall`, replace:
   ```bash
   install -Dm755 target/release/up-daemon $out/libexec/up-daemon
   ```
   with:
   ```bash
   mkdir -p $out/libexec
   mv $out/bin/up-daemon $out/libexec/up-daemon
   ```
4. Save and verify with `nix build` (requires a Linux system or NixOS CI)

---

## 9. Sources

1. **nixpkgs Rust manual** (`ryantm.github.io/nixpkgs/languages-frameworks/rust/`) — Documents `cargoBuildFlags` as the attribute for passing extra cargo build flags to `cargoBuildHook`; documents `cargoTestFlags` for `cargo test`; explicitly warns that artifacts go to `target/<architecture>/release/` not `target/release/` when `--target` is used.

2. **nixpkgs `cargo-build-hook.sh`** (GitHub raw, `master`) — Source code confirms `cargoBuildHook` always passes `--target @rustcTargetSpec@`; shows `concatTo flagsArray cargoBuildFlags` is the mechanism for injecting workspace flags.

3. **nixpkgs `cargo-install-hook.sh`** (GitHub raw, `master`) — Source code shows `releaseDir=target/@targetSubdirectory@/$cargoBuildType`, confirming the arch-specific path is used; shows `cargoInstallHook` copies executables to `$out/bin/` before calling `runHook postInstall`.

4. **nixpkgs `build-rust-package/default.nix`** (GitHub, `master`) — Confirms `CARGO_BUILD_TARGET` is always set from `rust.toRustTargetSpec stdenv.hostPlatform`; shows the full attribute surface including `cargoBuildFlags`.

5. **Context7 nixpkgs documentation** (`/nixos/nixpkgs`) — Provides authoritative API reference for `buildRustPackage` attributes including `buildFeatures`, `buildType`, `cargoBuildFlags`; confirms `cargoTestFlags` is the attribute for test-phase flags.

6. **nixpkgs Rust manual — "Running package tests"** — Explicitly states `checkFlags` and `cargoTestFlags` are distinct; documents that `--package foo` style flags can be passed to scope tests to specific workspace members.

7. **Cargo Book — Workspaces** (implicit via `cargo build --workspace` semantics) — `--workspace` (alias `--all`) instructs Cargo to build all packages in the workspace; without it, only the default members (typically the root crate) are built.

8. **nixpkgs manual — "Tests relying on the structure of the target/ directory"** — Directly documents that `target/release/` does not exist when `--target` is in effect; artifacts are in `target/<architecture>/release/` instead.
