# Quick Fixes Specification

**Feature Name:** quick_fixes  
**Date:** 2026-05-06  
**Scope:** URL corrections, dead code removal, CI optimisation, documentation reconciliation

---

## Summary of Changes

Six targeted fixes across five files:

| # | File | Issue | Action |
|---|------|--------|--------|
| 1 | `Cargo.toml` | Placeholder repository URL | Fix URL |
| 2 | `data/io.github.up.metainfo.xml` | Placeholder URLs (homepage + bugtracker) | Fix URLs |
| 3 | `src/ui/upgrade_page.rs` | `#[allow(dead_code)]` on `CheckMsg` enum + `Error` variant never constructed | Remove attribute, variant, and unreachable match arm |
| 4 | `.github/workflows/ci.yml` | `libunwind-dev` and `gettext` installed but unused | Remove both packages from apt-get line |
| 5 | `.github/workflows/ci.yml` | `cargo test --release` duplicates the release compile | Drop `--release` flag from test step only |
| 6 | `README.md` + `.github/docs/FLATPAK_CI_SUMMARY.md` | False claims about non-existent Flatpak CI files | Rewrite FLATPAK_CI_SUMMARY.md; remove Flatpak CI claims from README.md |

---

## Fix 1 — `Cargo.toml`: Placeholder Repository URL

### Current State

```toml
repository = "https://github.com/user/up"
```

Located at line 8 of `Cargo.toml`.

### Proposed Change

```toml
repository = "https://github.com/VictoryTek/Up"
```

### Risk Notes

- Low risk. Pure metadata field; no build or runtime impact.
- `crates.io` uses this field for the crate's web page link.

---

## Fix 2 — `data/io.github.up.metainfo.xml`: Placeholder URLs

### Current State

```xml
<url type="homepage">https://github.com/user/up</url>
<url type="bugtracker">https://github.com/user/up/issues</url>
```

Located at lines 22–23 of `data/io.github.up.metainfo.xml`.

### Proposed Change

```xml
<url type="homepage">https://github.com/VictoryTek/Up</url>
<url type="bugtracker">https://github.com/VictoryTek/Up/issues</url>
```

### Risk Notes

- Low risk. Metadata only; affects AppStream validation output and Flathub listings.
- `appstreamcli validate` will continue to pass with corrected URLs.

---

## Fix 3 — `src/ui/upgrade_page.rs`: Dead `CheckMsg::Error` Variant

### Current State

The enum is declared with `#[allow(dead_code)]` to silence a compiler warning about
the `Error(String)` variant, which is **never constructed** anywhere in the codebase.
The variant is matched in the receiver but can never be reached because no sender ever
sends it.

```rust
#[allow(dead_code)]
enum CheckMsg {
    /// A plain log line to display in the terminal output panel.
    Log(String),
    /// Structured results from all prerequisite checks.
    Results(Vec<upgrade::CheckResult>),
    /// A fatal error that prevented checks from completing.
    Error(String),
}
```

The unreachable match arm (lines 224–227 of `src/ui/upgrade_page.rs`):

```rust
                        CheckMsg::Error(e) => {
                            all_passed = false;
                            log_ref.append_line(&format!("Error: {e}"));
                        }
```

### Proposed Change

**Remove** the `#[allow(dead_code)]` attribute, the `Error(String)` variant and its
doc comment, and the corresponding match arm. The enum becomes:

```rust
enum CheckMsg {
    /// A plain log line to display in the terminal output panel.
    Log(String),
    /// Structured results from all prerequisite checks.
    Results(Vec<upgrade::CheckResult>),
}
```

The match block (beginning around line 203) becomes:

```rust
                    match msg {
                        CheckMsg::Log(line) => {
                            log_ref.append_line(&line);
                        }
                        CheckMsg::Results(results) => {
                            let rows = check_rows_ref.borrow();
                            let icons = check_icons_ref.borrow();
                            for (i, result) in results.iter().enumerate() {
                                if let Some(row) = rows.get(i) {
                                    row.set_subtitle(&result.message);
                                }
                                if let Some(icon) = icons.get(i) {
                                    if result.passed {
                                        icon.set_icon_name(Some("emblem-ok-symbolic"));
                                    } else {
                                        icon.set_icon_name(Some("dialog-error-symbolic"));
                                        all_passed = false;
                                    }
                                }
                            }
                        }
                    }
```

### Risk Notes

- Low risk. The `Error` variant was unreachable; removing it cannot alter runtime behaviour.
- `cargo clippy -- -D warnings` will now pass without the suppression attribute.
- If a future implementer wants error propagation from `run_prerequisite_checks`, they
  should re-add the variant and wire it to actual error return paths at that time.

---

## Fix 4 — `.github/workflows/ci.yml`: Remove Unused APT Packages

### Current State

```yaml
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
            desktop-file-utils \
            gettext \
            libunwind-dev
```

`gettext` and `libunwind-dev` are installed but are not referenced by any subsequent
step. `cargo build` does not require `libunwind-dev` on Ubuntu 24.04 (libunwind is
pulled transitively by system libraries already present). `gettext` is not used by
any step in the workflow.

### Proposed Change

```yaml
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
```

### Risk Notes

- Low risk. Both packages are unused. Removing them reduces CI install time by a few
  seconds and removes implicit dependency surface.
- If a future step requires `gettext` (e.g. for `.po` file compilation), it should be
  re-added at that time.

---

## Fix 5 — `.github/workflows/ci.yml`: Drop `--release` from Test Step

### Current State

```yaml
      - name: Build with cargo
        run: cargo build --release

      - name: Run cargo test
        run: cargo test --release
```

Both steps perform a release build. `cargo test --release` triggers a full second
release compilation pass (different codegen unit layout from `--release` build),
wasting CI minutes and providing no meaningful additional coverage — test semantics
are identical in debug vs release mode for this project.

### Proposed Change

```yaml
      - name: Build with cargo
        run: cargo build --release

      - name: Run cargo test
        run: cargo test
```

The build step validates the release binary; the test step runs tests against the
debug profile, which is faster and exercises the same logic.

### Risk Notes

- Low risk. Rust test semantics are the same in debug and release for this project
  (no `#[cfg(test)]` blocks gated on profile, no unsafe code relying on
  release-only optimisations).
- CI total time reduction: approximately 1–3 minutes depending on cache state.

---

## Fix 6 — Documentation Reconciliation: Flatpak CI Claims

### Current State

#### `.github/docs/FLATPAK_CI_SUMMARY.md`

The entire document declares "Implementation Complete ✅" and describes a working
Flatpak CI/CD pipeline. It references the following files that **do not exist** in
the repository:

- `.github/workflows/flatpak-ci.yml`
- `scripts/build-flatpak.sh`
- `scripts/verify-flatpak.sh`
- `io.github.up.json` (Flatpak manifest)

The document is misleading and will confuse contributors.

#### `README.md`

The following sections in `README.md` make false claims:

**In the "Development" section:**
```bash
# Build Flatpak locally
./scripts/build-flatpak.sh
```
This script does not exist.

**The "CI/CD" section:**
```
- **Flatpak Packaging**: Automatically builds the application as a Flatpak package
- **Release Automation**: Publishes Flatpak bundles to GitHub Releases on version tags
```
Neither is implemented.

**The code example in the CI/CD section:**
```bash
# Install GNOME 46 SDK and Rust extension
./scripts/build-flatpak.sh
```
This script does not exist.

**The entire "# Flatpak Build (Premium Release Process)" section** at the bottom of
`README.md` describes a build/release flow that does not exist.

---

### Proposed Change: `FLATPAK_CI_SUMMARY.md`

Replace the entire file content with a brief, honest status document:

```markdown
# Flatpak CI/CD — Status

## Status: Planned (Not Yet Implemented)

A Flatpak CI/CD pipeline for the **Up** application is planned but has not yet
been implemented.

## What is planned

- A Flatpak manifest (`io.github.up.json`)
- A GitHub Actions workflow (`.github/workflows/flatpak-ci.yml`) that builds and
  tests the application as a Flatpak on each push and pull request
- Helper scripts (`scripts/build-flatpak.sh`, `scripts/verify-flatpak.sh`) for
  local Flatpak development
- Automated GitHub Release asset generation on version tags
- Eventual Flathub submission

## Current Installation Methods

Until Flatpak packaging is complete, the application can be installed via:

- **Nix Flake:** `nix run github:VictoryTek/Up`
- **From source:** `cargo build --release` (see README.md for full instructions)

## Contributing

If you would like to help implement Flatpak packaging, please open an issue or pull
request at https://github.com/VictoryTek/Up.
```

---

### Proposed Change: `README.md`

Three targeted edits:

#### Edit A — Remove false Flatpak build command from "Development" section

**Remove** the following two lines from the Development section:
```
# Build Flatpak locally
./scripts/build-flatpak.sh
```
(The blank line and preceding comment should also be removed.)

#### Edit B — Replace the "CI/CD" section body

**Current:**
```markdown
## CI/CD

The project uses GitHub Actions for continuous integration and deployment:

- **Build Testing**: Runs cargo fmt, clippy, build, and test on all pull requests and pushes to main
- **Flatpak Packaging**: Automatically builds the application as a Flatpak package
- **Release Automation**: Publishes Flatpak bundles to GitHub Releases on version tags

To manually test the Flatpak CI, you can run:
```bash
# Install GNOME 46 SDK and Rust extension
./scripts/build-flatpak.sh
```
```

**Replace with:**
```markdown
## CI/CD

The project uses GitHub Actions for continuous integration:

- **Build Testing**: Runs `cargo fmt`, `cargo clippy`, `cargo build`, and `cargo test` on all pull requests and pushes to `main`
- **Validation**: Validates the desktop file and AppStream metadata on every run

Flatpak packaging and automated release assets are planned for a future release.
```

#### Edit C — Remove the entire "Flatpak Build (Premium Release Process)" section

The entire block starting at `# Flatpak Build (Premium Release Process)` through the
end of the file should be removed. This includes:
- The heading and introductory paragraph
- The "Automated Release with GitHub Actions" sub-section
- The "Manual Flatpak Build" sub-section
- The "System Requirements" sub-section and its reference to `FLATPAK_README.md`

### Risk Notes

- Low risk for documentation changes. No code is altered.
- `README.md` will still accurately describe the Nix and source install paths.
- The `FLATPAK_CI_SUMMARY.md` rewrite removes misleading status claims; no existing
  functionality depends on this document.
- The reference to `.github/docs/FLATPAK_README.md` in the removed section is also
  eliminated — that file may or may not exist, but it is no longer referenced.

---

## Files to Modify

| File | Change Type |
|------|-------------|
| `Cargo.toml` | URL fix (1 line) |
| `data/io.github.up.metainfo.xml` | URL fix (2 lines) |
| `src/ui/upgrade_page.rs` | Remove attribute, enum variant, doc comment, match arm |
| `.github/workflows/ci.yml` | Remove 2 apt packages; remove `--release` from test step |
| `README.md` | Remove/replace 3 sections of Flatpak CI claims |
| `.github/docs/FLATPAK_CI_SUMMARY.md` | Full rewrite |

---

## Implementation Notes

1. All edits are independent — they can be applied in any order.
2. After applying Fix 3 (dead code removal), run `cargo clippy -- -D warnings` to
   confirm the warning is gone without the suppression attribute.
3. After applying Fix 4 and Fix 5, the CI workflow should be manually inspected to
   ensure no trailing backslash issues remain in the apt-get block.
4. The `#[allow(dead_code)]` on the enum was suppressing only the `Error` variant
   warning. The `Log` and `Results` variants are actively used and will not generate
   warnings once the attribute is removed.
5. No new dependencies are introduced by any of these changes.
6. No Meson, Nix, or Flatpak build system changes are required.
