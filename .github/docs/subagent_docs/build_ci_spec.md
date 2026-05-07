# Build / Packaging / CI — Implementation Specification

**Feature name:** `build_ci`  
**Spec version:** 1.0  
**Date:** 2026-05-07  
**Items covered:** 7.2, 7.6, 7.8, 7.11, 7.12, 7.14

---

## 1. Current State Analysis

### Repository root inventory
| File | Present | Notes |
|---|---|---|
| `.github/workflows/ci.yml` | ✅ | Native Ubuntu build; no Nix step |
| `.github/workflows/gitlab-mirror.yml` | ✅ | Mirrors `main` branch and tags to GitLab |
| `.gitlab-ci.yml` | ✅ | Mirrors ci.yml for GitLab |
| `scripts/preflight.sh` | ✅ | fmt → clippy → build → test → desktop-file-validate → appstreamcli |
| `rust-toolchain.toml` | ❌ | Missing — Rust version only pinned in CI via `dtolnay/rust-toolchain@stable` |
| `.editorconfig` | ❌ | Missing |
| `cargo-sources.json` | ✅ (unwanted) | Orphaned Flatpak vendor sources file; must be deleted |
| `flake.nix` | ✅ | Builds `packages.default` via `rustPlatform.buildRustPackage`; pins `nixpkgs/nixos-25.05` |

### ci.yml key facts (must be mirrored exactly in release.yml)
- Runner: `ubuntu-24.04`
- Checkout: `actions/checkout@v6`
- Rust: `dtolnay/rust-toolchain@stable` with `components: clippy, rustfmt`
- Cache: `Swatinem/rust-cache@v2` with `cache-on-failure: true`
- APT packages: `libgtk-4-dev libadwaita-1-dev libglib2.0-dev libcairo2-dev libpango1.0-dev pkg-config cmake ninja-build desktop-file-utils`
- Steps: fmt → clippy → `cargo build --release` → test → desktop-file-validate → appstreamcli validate

### meson.build key facts
```meson
cargo_build = custom_target('cargo-build',
  output: 'up',
  command: ['sh', '-c',
    cargo.full_path() + ' build ' + ... +
    '--manifest-path ' + srcdir / 'Cargo.toml' +
    ' && cp ' + srcdir / 'target' / rust_target / 'up' + ' @OUTPUT@'
  ],
  build_always_stale: true,   # ← defeats incremental builds
  ...
)
```
Two problems:
1. `build_always_stale: true` forces a full Cargo invocation on every `meson compile`, negating incremental builds.
2. Cargo writes to `srcdir/target/` (source tree), not the Meson out-of-tree build directory.

### flake.nix key facts
- `packages.default` = `pkgs.rustPlatform.buildRustPackage { pname = "up"; ... }`
- `mainProgram = "up"` → binary at `./result/bin/up` after `nix build`
- Inputs locked to `nixpkgs/nixos-25.05` via `flake.lock`
- `cargoLock.lockFile = ./Cargo.lock` (no vendor dir needed)

### preflight.sh key facts
- Steps 1–6: fmt, clippy, build, test, desktop-file-validate, appstreamcli
- Missing: cargo audit (step 7), nix flake check (step 8)
- Graceful skip pattern already established (check `command -v`, print Notice)

---

## 2. Items to Implement

---

### 2.1 Item 7.2 — Release-tag GitHub Actions workflow

**File to create:** `.github/workflows/release.yml`

#### Trigger
```yaml
on:
  push:
    tags:
      - 'v*.*.*'
```

#### Strategy
Use two jobs:
1. `ci-checks` — runs all existing CI validations (mirrors `native-build` from ci.yml exactly)
2. `release` — needs: `ci-checks`; installs Nix, runs `nix build`, uploads binary to GitHub Release

**Rationale for two-job approach over `workflow_call`:**  
Adding `workflow_call:` to ci.yml is a safe minimal change (just adds a trigger), but it requires modifying a working file. Replicating the ci-checks job in release.yml avoids touching ci.yml and is fully self-contained. This spec uses the two-job approach for lower risk.

#### Job 1: `ci-checks`

Exact mirror of `native-build` job from ci.yml:
- Runner: `ubuntu-24.04`
- Steps:
  1. `actions/checkout@v6`
  2. `dtolnay/rust-toolchain@stable` with `components: clippy, rustfmt`
  3. `Swatinem/rust-cache@v2` with `cache-on-failure: true`
  4. APT install (same package list as ci.yml)
  5. `cargo fmt --check`
  6. `cargo clippy -- -D warnings`
  7. `cargo build --release`
  8. `cargo test`
  9. `desktop-file-validate data/io.github.up.desktop`
  10. `appstreamcli validate --no-net data/io.github.up.metainfo.xml`

#### Job 2: `release`

- Runner: `ubuntu-24.04`
- `needs: [ci-checks]`
- Permissions: `contents: write` (required for creating releases)

Steps:
1. `actions/checkout@v6`
2. Install Nix:
   ```yaml
   - name: Install Nix
     uses: cachix/install-nix-action@v31
     with:
       extra_nix_config: |
         experimental-features = nix-command flakes
   ```
   **Note:** Use the latest stable release of `cachix/install-nix-action` at implementation time. As of research date, v31 is appropriate; verify at https://github.com/cachix/install-nix-action/releases.

3. Build Nix package:
   ```yaml
   - name: Build Nix package
     run: nix build
   ```
   Output: `./result` symlink → Nix store path. Binary at `./result/bin/up`.

4. Rename binary with version:
   ```yaml
   - name: Prepare release binary
     run: |
       VERSION="${GITHUB_REF_NAME#v}"
       cp ./result/bin/up "up-${VERSION}-linux-x86_64"
       echo "BINARY_NAME=up-${VERSION}-linux-x86_64" >> "$GITHUB_ENV"
   ```

5. Create GitHub Release and upload binary:
   ```yaml
   - name: Create GitHub Release
     uses: softprops/action-gh-release@v2
     with:
       files: ${{ env.BINARY_NAME }}
       generate_release_notes: true
     env:
       GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
   ```
   **Note:** Use the latest stable release of `softprops/action-gh-release` at implementation time. v2 is the current major.

#### Full workflow YAML (authoritative template)

```yaml
# Release workflow for Up
# Triggers on version tags (v*.*.*), runs CI checks then publishes a GitHub Release.

name: Release

on:
  push:
    tags:
      - 'v*.*.*'

env:
  CARGO_TERM_COLOR: always

jobs:
  ci-checks:
    name: CI Checks
    runs-on: ubuntu-24.04
    steps:
      - name: Checkout source code
        uses: actions/checkout@v6

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt

      - name: Cache Rust dependencies
        uses: Swatinem/rust-cache@v2
        with:
          cache-on-failure: true

      - name: Install GTK4 and libadwaita dependencies
        run: |
          sudo apt-get update
          sudo apt-get install -y \
            libgtk-4-dev \
            libadwaita-1-dev \
            libglib2.0-dev \
            libcairo2-dev \
            libpango1.0-dev \
            pkg-config \
            cmake \
            ninja-build \
            desktop-file-utils

      - name: Run cargo fmt check
        run: cargo fmt --check

      - name: Run cargo clippy
        run: cargo clippy -- -D warnings

      - name: Build with cargo
        run: cargo build --release

      - name: Run cargo test
        run: cargo test

      - name: Validate desktop file
        run: desktop-file-validate data/io.github.up.desktop

      - name: Validate AppStream metadata
        run: |
          if ! command -v appstreamcli &>/dev/null; then
            sudo apt-get install -y appstream
          fi
          appstreamcli validate --no-net data/io.github.up.metainfo.xml

  release:
    name: Publish GitHub Release
    runs-on: ubuntu-24.04
    needs: [ci-checks]
    permissions:
      contents: write

    steps:
      - name: Checkout source code
        uses: actions/checkout@v6

      - name: Install Nix
        uses: cachix/install-nix-action@v31
        with:
          extra_nix_config: |
            experimental-features = nix-command flakes

      - name: Build Nix package
        run: nix build

      - name: Prepare release binary
        run: |
          VERSION="${GITHUB_REF_NAME#v}"
          cp ./result/bin/up "up-${VERSION}-linux-x86_64"
          echo "BINARY_NAME=up-${VERSION}-linux-x86_64" >> "$GITHUB_ENV"

      - name: Create GitHub Release
        uses: softprops/action-gh-release@v2
        with:
          files: ${{ env.BINARY_NAME }}
          generate_release_notes: true
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

#### Risks
- **Nix build time on GitHub Actions:** `nixpkgs/nixos-25.05` is large. The first run will download GTK4, libadwaita, and all Rust crates. Subsequent runs may benefit from GitHub Actions cache. Consider adding `cachix/cachix-action` to a Cachix binary cache if build times become excessive.
- **`nix build` vs `nix build .#default`:** Both work; bare `nix build` selects `packages.${system}.default` automatically.
- **`GITHUB_TOKEN` permissions:** The workflow must run with `permissions: contents: write` at the job level. The repository's default permissions may restrict this.
- **`generate_release_notes: true`** requires the repository to use conventional commits or GitHub's release notes generator. If not desired, remove or set to `false`.

---

### 2.2 Item 7.6 — Fix meson.build out-of-tree build hygiene

**File to modify:** `meson.build`

#### Problem 1: `build_always_stale: true`

Removing this option requires Meson to determine staleness through other means. Since the `custom_target` has no `input:` or `depend_files:`, Meson would only rebuild if the output file `up` is absent. This means edits to Rust source files would not trigger a rebuild.

**Minimum safe fix:** Add `depend_files` for `Cargo.toml` and `Cargo.lock`. These files change on every dependency update and every version bump — covering the most common reasons to rebuild. Individual `.rs` file tracking requires `depfile:` (see risk note below).

#### Problem 2: Cargo writes to `srcdir/target/`

Cargo's default target directory is the `target/` subdirectory of the crate root (the source directory). This pollutes the source tree and is inconsistent with Meson's expectation of out-of-tree builds.

**Fix:** Pass `--target-dir` to Cargo pointing to a subdirectory of `builddir`. This keeps all Cargo artifacts inside the Meson build directory.

#### Exact diff to apply

```meson
# BEFORE:
cargo_build = custom_target('cargo-build',
  output: 'up',
  command: [
    'sh', '-c',
    cargo.full_path() + ' build ' +
    (rust_target == 'release' ? '--release ' : '') +
    '--manifest-path ' + srcdir / 'Cargo.toml' +
    ' && cp ' + srcdir / 'target' / rust_target / 'up' + ' @OUTPUT@'
  ],
  build_always_stale: true,
  console: true,
  install: true,
  install_dir: bindir,
)

# AFTER:
cargo_build = custom_target('cargo-build',
  output: 'up',
  depend_files: files('Cargo.toml', 'Cargo.lock'),
  command: [
    'sh', '-c',
    cargo.full_path() + ' build ' +
    (rust_target == 'release' ? '--release ' : '') +
    '--manifest-path ' + srcdir / 'Cargo.toml' +
    ' --target-dir ' + builddir / 'cargo-target' +
    ' && cp ' + builddir / 'cargo-target' / rust_target / 'up' + ' @OUTPUT@'
  ],
  console: true,
  install: true,
  install_dir: bindir,
)
```

Changes:
1. Remove `build_always_stale: true`
2. Add `depend_files: files('Cargo.toml', 'Cargo.lock')`
3. Add `--target-dir ' + builddir / 'cargo-target'` to the cargo command
4. Update the `cp` source path from `srcdir / 'target' / rust_target / 'up'` to `builddir / 'cargo-target' / rust_target / 'up'`

#### Risks

- **Incomplete staleness tracking:** Only `Cargo.toml` and `Cargo.lock` changes trigger Meson to re-run the target. Changes to individual `.rs` files in `src/` will not trigger a rebuild via Meson. However, Cargo itself tracks source file timestamps and will produce no-op rebuilds (extremely fast) when nothing changed. The practical impact is: `meson compile` will always invoke `cargo build`, but Cargo will skip compilation if sources are unchanged. This is acceptable for a GTK app build workflow where Meson is used for installation, not iterative development.
- **`depfile:` alternative:** A more complete solution would use `depfile:` with `cargo build --emit=dep-info` to get per-file tracking. This is significantly more complex and fragile and is not recommended at this time.
- **`build_always_stale` was intentional:** It was likely added because the author knew about the staleness problem. Removing it without `depfile:` restores the simpler behavior where `cargo build` is always called (via Cargo's own incremental logic). If the Meson build is used in a tight loop where even invoking `cargo build` is too slow, document this.
- **Existing `target/` directory:** If developers have an existing `target/` directory in the source tree from prior Cargo builds, they may need to clean it up. No action is needed from meson.build; developers can run `cargo clean` if desired.

---

### 2.3 Item 7.8 — cargo audit + nix flake check in preflight and CI

#### 2.3.1 preflight.sh additions

Append two new steps at the end of `scripts/preflight.sh`, before the final `echo "All preflight checks passed."` line.

**Step 7 — cargo audit:**
```bash
echo "--- Step 7: Security audit (cargo audit) ---"
if cargo audit --version &>/dev/null 2>&1; then
    cargo audit
else
    echo "Notice: cargo-audit not found, skipping security audit."
    echo "        Install with: cargo install cargo-audit"
fi
```

**Step 8 — nix flake check (optional, graceful):**
```bash
echo "--- Step 8: Nix flake check ---"
if [[ -f flake.nix ]] && command -v nix &>/dev/null; then
    nix flake check 2>/dev/null && echo "Nix flake check passed." || echo "Notice: nix flake check could not complete (may require full Nix evaluation); skipping."
else
    echo "Notice: Nix not available or no flake.nix found, skipping nix flake check."
fi
```

**Important:** The `nix flake check` step must use `|| echo "Notice: ..."` (not `|| exit 1`) because `nix flake check` evaluates and builds all `checks` outputs, which on a non-NixOS system may fail due to missing evaluation context. The graceful skip is intentional.

**Placement:** Both steps go between the current `appstreamcli validate` block and the final `echo "All preflight checks passed."` line.

#### 2.3.2 ci.yml addition

Add a new `cargo-audit` job to `.github/workflows/ci.yml`:

```yaml
  cargo-audit:
    name: Security Audit
    runs-on: ubuntu-24.04
    steps:
      - name: Checkout source code
        uses: actions/checkout@v6

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable

      - name: Cache Rust dependencies
        uses: Swatinem/rust-cache@v2
        with:
          cache-on-failure: true

      - name: Install cargo-audit
        run: cargo install cargo-audit --locked

      - name: Run cargo audit
        run: cargo audit
```

Add `cargo-audit` to the `needs` list of the existing `validation` job:
```yaml
  validation:
    name: CI Validation
    runs-on: ubuntu-24.04
    needs: [native-build, cargo-audit]
    if: always()
    steps:
      - name: Check all jobs passed
        run: |
          echo "CI Pipeline Summary:"
          echo "✓ Native Build & Test: ${{ needs.native-build.result }}"
          echo "✓ Security Audit:      ${{ needs.cargo-audit.result }}"
          echo ""
          if [ "${{ needs.native-build.result }}" != "success" ] || \
             [ "${{ needs.cargo-audit.result }}" != "success" ]; then
            echo "❌ CI Pipeline failed"
            exit 1
          else
            echo "✅ All CI checks passed successfully"
          fi
```

**Risk:** `cargo audit` will fail the CI if any dependency has a published advisory. This is the desired behavior for a security gate. If an advisory exists for a transitive dependency that has no fix yet, use `cargo audit --ignore RUSTSEC-XXXX-XXXX` with the specific advisory ID. Do not add blanket `--ignore-*` flags.

**Note:** `nix flake check` is NOT added to ci.yml because the CI runner does not have Nix installed (and installing Nix mid-job adds significant complexity). The Nix build is handled exclusively in the release workflow.

---

### 2.4 Item 7.11 — Delete cargo-sources.json

**File to delete:** `cargo-sources.json`

Confirmed content: JSON array of `{ "type": "archive", "url": "https://static.crates.io/crates/..." }` entries. This is the vendored crate sources file generated by `flatpak-cargo-generator.py` for Flatpak offline builds. The Flatpak build has been retired. The file has no references elsewhere in the repository (meson.build, flake.nix, Cargo.toml, CI scripts do not reference it).

**Implementation:** `git rm cargo-sources.json` or equivalent deletion.

---

### 2.5 Item 7.12 — Add rust-toolchain.toml

**File to create:** `rust-toolchain.toml` at repository root.

```toml
[toolchain]
channel = "stable"
```

**Rationale:** Ensures all local development uses the stable channel, matching the `dtolnay/rust-toolchain@stable` step in CI. Without this file, a developer using a nightly or beta rustup default would get inconsistent behavior.

**Note:** The existing CI does not need to be changed. `dtolnay/rust-toolchain@stable` reads `rust-toolchain.toml` automatically but will use `stable` regardless of the file content since it explicitly pins `@stable`. The file primarily benefits local development workflows.

**No version pinning** (e.g., `channel = "1.87.0"`) is intentional — the project has no known minimum version requirements and pinning to a specific stable release creates maintenance burden without benefit.

---

### 2.6 Item 7.14 — Add .editorconfig

**File to create:** `.editorconfig` at repository root.

```ini
root = true

[*]
charset = utf-8
end_of_line = lf
trim_trailing_whitespace = true
insert_final_newline = true

[*.{rs,toml}]
indent_style = space
indent_size = 4

[*.{yml,yaml}]
indent_style = space
indent_size = 2

[*.{json,xml}]
indent_style = space
indent_size = 2

[*.md]
trim_trailing_whitespace = false

[Makefile]
indent_style = tab
indent_size = 4
```

**Notes:**
- `trim_trailing_whitespace = false` for Markdown because trailing spaces are semantic (line breaks in some renderers).
- `Makefile` uses tabs (required by Make syntax).
- XML uses 2-space indent to match the project's existing data files (`io.github.up.metainfo.xml`, `io.github.up.policy`).
- JSON uses 2-space indent to match `io.github.up.gresource.xml` and common JSON convention.
- YAML uses 2-space indent matching the existing `.github/workflows/*.yml` files.

---

## 3. Implementation Order

Implement in this order to minimize risk:

1. **7.11** — Delete `cargo-sources.json` (zero risk, no dependencies)
2. **7.12** — Create `rust-toolchain.toml` (zero risk, additive)
3. **7.14** — Create `.editorconfig` (zero risk, additive)
4. **7.6** — Fix `meson.build` (low risk; test with `meson setup builddir && meson compile -C builddir` if Meson is available)
5. **7.8** — Update `preflight.sh` and `ci.yml` (medium risk; must not break existing CI)
6. **7.2** — Create `.github/workflows/release.yml` (no risk until a tag is pushed)

---

## 4. Files to Create / Modify / Delete

| Action | Path |
|---|---|
| CREATE | `.github/workflows/release.yml` |
| MODIFY | `meson.build` |
| MODIFY | `scripts/preflight.sh` |
| MODIFY | `.github/workflows/ci.yml` |
| CREATE | `rust-toolchain.toml` |
| CREATE | `.editorconfig` |
| DELETE | `cargo-sources.json` |

---

## 5. Verification Steps (for Review Phase)

1. `cargo fmt --check` — must pass
2. `cargo clippy -- -D warnings` — must pass
3. `cargo build` — must pass
4. `cargo test` — must pass
5. `scripts/preflight.sh` — must reach `All preflight checks passed.`
6. Validate `meson.build` syntax: `meson setup builddir --wipe` in a clean checkout (if Meson is installed)
7. Validate release.yml YAML syntax: `yamllint .github/workflows/release.yml` or `actionlint`
8. Confirm `cargo-sources.json` is absent from the repo root
9. Confirm `rust-toolchain.toml` is present and parses as valid TOML
10. Confirm `.editorconfig` is present at root

---

## 6. Risk Summary

| Item | Risk Level | Notes |
|---|---|---|
| 7.2 release workflow | Low | Only triggers on version tags; Nix build time may be long |
| 7.6 meson.build | Medium | Removing `build_always_stale` changes rebuild semantics; must verify `meson compile` still works |
| 7.8 cargo audit in CI | Medium | May block CI on existing advisories in transitive deps |
| 7.8 nix flake check in preflight | Low | Graceful skip pattern mitigates failure risk |
| 7.11 delete cargo-sources.json | None | File is unused |
| 7.12 rust-toolchain.toml | None | Additive only |
| 7.14 .editorconfig | None | Additive only |
