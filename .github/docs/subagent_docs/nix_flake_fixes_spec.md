# Nix Flake Fixes — Specification

## Current State of `flake.nix`

The file defines a single Nix flake output for the `up` package using
`pkgs.rustPlatform.buildRustPackage` plus a `devShells.default` shell.

Key structural points:

- `inputs.nixpkgs` is pinned to the **floating** branch `nixos-unstable`.
- `nativeBuildInputs` contains only `pkg-config` and `wrapGAppsHook4`.
- `buildInputs` contains `gtk4`, `libadwaita`, `glib`, `dbus`, and
  `hicolor-icon-theme`.
- `postInstall` installs the desktop file, metainfo XML, and the
  `256x256` icon, then calls `gtk4-update-icon-cache`.
- `meta` block contains `description`, `license`, `platforms`, and
  `mainProgram` — but is **missing `homepage`**.

---

## Findings

### Icon file availability

Directory tree under `data/icons/`:

```
data/icons/
└── hicolor/
    └── 256x256/
        └── apps/
            └── io.github.up.png
```

Only the `256x256` size is present. `128x128` and `48x48` do **not**
exist as physical files. `meson.build` already guards icon installation
with `fs.exists(png)`, so it silently skips missing sizes. No change to
`postInstall` for additional icon sizes is required.

### Homepage URL

Extracted from `Cargo.toml`:

```
repository = "https://github.com/user/up"
```

This is the value to use for `meta.homepage`.

### nixpkgs stable branch

Current date: **April 1, 2026**.  
`nixos-25.05` was released approximately May 2025 and is confirmed
available. The branch to pin to is **`nixos-25.05`**.

A `nix flake update` after applying this change will resolve the exact
commit hash and write it to `flake.lock`, providing full reproducibility.
No manual commit hash is required in the spec.

---

## Changes Required

### Change 1 — Pin nixpkgs to a stable branch

**File:** `flake.nix`  
**Rationale:** `nixos-unstable` is a rolling, mutable pointer. Pinning
to a stable release branch (combined with `flake.lock`) ensures
reproducible builds. `nixos-25.05` is the latest stable branch as of
April 2026.

**Old text:**
```nix
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
```

**New text:**
```nix
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
```

---

### Change 2 — Add `glib` and `gtk4` to `nativeBuildInputs`

**File:** `flake.nix`  
**Rationale:**

- `glib` provides `glib-compile-resources`, which `build.rs` (via the
  `glib-build-tools` crate) invokes at **compile time**. In Nix,
  compile-time tools must reside in `nativeBuildInputs` so they are
  available in the build environment and, importantly, are the
  host-architecture binaries when cross-compiling.
- `gtk4` provides `gtk4-update-icon-cache`, which is called in
  `postInstall`. `postInstall` executes in the build environment, so
  the binary must come from `nativeBuildInputs`.

Both packages remain in `buildInputs` as well (their runtime libraries
are needed there). Adding them to `nativeBuildInputs` in addition is
correct and idiomatic Nix practice.

**Old text:**
```nix
          nativeBuildInputs = with pkgs; [
            pkg-config
            wrapGAppsHook4
          ];
```

**New text:**
```nix
          nativeBuildInputs = with pkgs; [
            pkg-config
            wrapGAppsHook4
            glib
            gtk4
          ];
```

---

### Change 3 — Add `homepage` to `meta` block

**File:** `flake.nix`  
**Rationale:** The `meta.homepage` attribute is a standard Nix metadata
field used by `nix-env`, `nix search`, and Nixpkgs tooling to surface
the upstream project URL. Its absence is flagged as a linting warning
by `nixpkgs-review` and related tools.

**Old text:**
```nix
          meta = with pkgs.lib; {
            description = "A modern Linux system update & upgrade app";
            license = licenses.gpl3Plus;
            platforms = platforms.linux;
            mainProgram = "up";
          };
```

**New text:**
```nix
          meta = with pkgs.lib; {
            description = "A modern Linux system update & upgrade app";
            homepage = "https://github.com/user/up";
            license = licenses.gpl3Plus;
            platforms = platforms.linux;
            mainProgram = "up";
          };
```

---

## Changes NOT Required

| Item | Reason |
|------|--------|
| Add `128x128` icon to `postInstall` | File does not exist in the repository (`data/icons/hicolor/128x128/` is absent). |
| Add `48x48` icon to `postInstall` | File does not exist in the repository (`data/icons/hicolor/48x48/` is absent). |

---

## Implementation Order

Apply changes in this order to avoid merge conflicts:

1. Change 1: pin nixpkgs URL (line 4 of `flake.nix`)
2. Change 2: expand `nativeBuildInputs` (lines 25–28 of `flake.nix`)
3. Change 3: add `homepage` to `meta` (lines 59–64 of `flake.nix`)

After applying, run `nix flake update` to regenerate `flake.lock` with
the pinned commit for `nixos-25.05`.

---

## Post-Implementation Validation

```bash
nix flake check
nix build
```

Both commands must complete without errors.
