# Flatpak CI/CD README

## Overview

The **Up** application uses GitHub Actions to automatically build, test, and package the application as a Flatpak. This document provides a comprehensive guide to the CI/CD workflow, local development, and release processes.

## CI/CD Workflow Structure

The workflow consists of the following components:

```
.github/workflows/
└── flatpak-ci.yml     # Main CI/CD workflow
```

### Workflow Triggers

The CI/CD workflow runs automatically when:
- Code is pushed to the `main` branch
- Pull requests are created or updated
- Git tags are pushed (for releases)

### Workflow Jobs

1. **Native Build & Test** (3-5 minutes)
   - Checkout source code
   - Install Rust toolchain
   - Cache Rust dependencies
   - Run format checks (`cargo fmt --check`)
   - Run lint checks (`cargo clippy -- -D warnings`)
   - Build the application (`cargo build`)
   - Execute unit tests (`cargo test`)
   - Validate desktop files (`desktop-file-validate`)
   - Validate AppStream metadata (`appstreamcli validate`)

2. **Flatpak Build** (10-15 minutes)
   - Checkout source code
   - Set up GNOME 46 SDK and Platform
   - Install Rust GNOME extension
   - Build the Flatpak package
   - Validate the Flatpak bundle
   - Upload to GitHub Releases (tags only)

3. **CI Validation** (30 seconds)
   - Aggregate all job results
   - Display build status summary
   - Exit with appropriate status code

## Local Development

### Prerequisites

Before building locally, ensure you have these tools installed:

```bash
# Verify Flatpak and flatpak-tools
flatpak --version
flatpak-builder --version

# Required GNOME 46 SDK components should be auto-downloaded
```

### Quick Start

The simplest way to build the Flatpak locally:

```bash
# Clone the repository
git clone https://github.com/user/up.git
cd up

# Build the Flatpak package
./scripts/build-flatpak.sh

# Run the application
flatpak run io.github.up
```

### Manual Build Steps

For more control, you can build step-by-step:

```bash
# 1. Install GNOME SDK and Rust extension
flatpak install -y flathub org.gnome.Platform//46 org.gnome.Sdk//46
flatpak install -y flathub org.freedesktop.Sdk.Extension.rust-stable//24.08

# 2. Build the Flatpak package
flatpak-builder --user --install --force-clean builddir io.github.up.json

# 3. Create a standalone bundle for distribution
flatpak build-bundle --unate builddir/repo up-release.flatpak io.github.up
```

## Release Process

### Preparing a Release

1. Update the version in `Cargo.toml` and `meson.build`:

```toml
[package]
name = "up"
version = "1.0.0"  # Update this
```

2. Update the metainfo file with release notes:

```xml
<releases>
  <release version="1.0.0" date="2024-12-01">
    <description>Initial release</description>
  </release>
</releases>
```

3. Commit and tag the release:

```bash
git commit -am "Release version 1.0.0"
git tag -a v1.0.0 -m "Release version 1.0.0"
git push origin v1.0.0
```

4. The CI/CD workflow will automatically:
   - Build the Flatpak package
   - Create a GitHub Release
   - Attach the Flatpak bundle as a release asset

### Manual Release (Optional)

If you prefer to create releases manually:

```bash
# 1. Build the Flatpak
flatpak-builder --user --install --force-clean builddir io.github.up.json

# 2. Create the bundle
flatpak build-bundle --unate builddir/repo up-release.flatpak io.github.up

# 3. Upload to GitHub Releases
# Navigate to: https://github.com/user/up/releases
# Create a new release and attach the .flatpak file
```

## Troubleshooting

### Common Issues

#### Issue: GNOME 46 SDK Not Found

```
Error: While looking at 'org.gnome.Sdk': No such file org.gnome.Sdk version 46
```

**Solution:**

```bash
flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub
flatpak install -y flathub org.gnome.Sdk//46
```

#### Issue: Rust Extension Installation Failed

```
Error: No remote sources found for runtime org.freedesktop.Sdk.Extension.rust-stable
```

**Solution:**

```bash
flatpak install -y flathub org.freedesktop.Sdk.Extension.rust-stable//24.08
```

#### Issue: Build Failures in CI/CD

If the GitHub Actions workflow fails:

1. Check the Actions tab on GitHub
2. Download the workflow logs
3. Look for error messages in the `flatpak-builder` step
4. Common fixes:
   - Update the Flatpak manifest (`io.github.up.json`)
   - Ensure all dependencies are declared
   - Verify the GNOME SDK version matches (46)

#### Issue: Desktop File Validation Errors

```
Error: desktop-file-validate reports errors:
- Key 'Categories' not found in group 'Desktop Entry'
```

**Solution:**

```bash
# Fix the desktop file
nano data/io.github.up.desktop

# Add required fields (then rebuild the Flatpak)
Categories=Utility;System;
```

#### Issue: AppStream Metadata Validation Warnings

```
Warning: Value '...' for attribute 'type' is deprecated. Use 'generic' instead.
```

**Solution:**

```xml
<!-- Fix in data/io.github.up.metainfo.xml -->
<id type="desktop">io.github.up</id>  <!-- Change to: <id>io.github.up</id> -->
```

### Debugging Flatpak Builds

For verbose build output:

```bash
# Enable debug logging
FLATPAK_BUILDER_FORCE_SHOW_TRACE=True flatpak-builder \
  --user \
  --install \
  --force-clean \
  --show trace \
  builddir io.github.up.json
```

### Performance Optimization

The CI/CD workflow uses caching to speed up subsequent builds:

- `.cargo` directory (Rust dependencies)
- `target` directory (build outputs)
- `builddir` directory (Flatpak build artifacts)

For local builds, caching is automatic. For CI/CD, the workflow uses GitHub Actions caching.

### Testing Downloads from Actions

To download artifacts from GitHub Actions:

```bash
# Install GH CLI (if not installed)
# Ubuntu/Debian:
# sudo apt install gh

# Download artifacts from a workflow run
gh run download --repo user/up --name flatpak-bundle

# Run the downloaded Flatpak
flatpak install up-release.flatpak
flatpak run io.github.up
```

## Advanced Configuration

### Modifying the Flatpak Manifest

The config files located in `io.github.up.json`:

**Important fields:**

- `app-id`: The unique identifier (e.g., `io.github.up`)
- `runtime`: GNOME platform version (e.g., `org.gnome.Platform//46`)
- `sdk`: GNOME SDK version (e.g., `org.gnome.Sdk//46`)
- `sdk-extensions`: Additional SDK components (e.g., Rust extension)
- `finish-args`: Permissions and sandboxing settings

Example manifest snippet:

```json
{
    "app-id": "io.github.up",
    "runtime": "org.gnome.Platform",
    "runtime-version": "46",
    "sdk": "org.gnome.Sdk",
    "sdk-extensions": [
        "org.freedesktop.Sdk.Extension.rust-stable"
    ],
    "command": "up",
    "build-options": {
        "append-path": "/usr/lib/sdk/rust-stable/bin",
        "env": {
            "CARGO_HOME": "/run/build/up/cargo"
        }
    },
    "modules": [
        {
            "name": "up",
            "buildsystem": "meson",
            "config-opts": [
                "-Dbuildtype=release"
            ],
            "sources": [
                {
                    "type": "dir",
                    "path": "."
                }
            ]
        }
    ]
}
```

### Updating the GNOME SDK Version

To upgrade from GNOME 46 to a newer version (e.g., GNOME 47):

```bash
# 1. Update io.github.up.json
# Change all references from "46" to "47"

# 2. Update .github/workflows/flatpak-ci.yml
# Change all references from "46" to "47"

# 3. Test locally
./scripts/build-flatpak.sh

# 4. Commit and push changes
git commit -am "Update to GNOME SDK 47"
git push origin main
```

## Security & Permissions

The Flatpak manifest defines the app's permissions through `finish-args`:

```json
"finish-args": [
    "--share=ipc",
    "--socket=fallback-x11",
    "--socket=wayland",
    "--filesystem=host:reset"
]
```

### Sandboxing Explanation

The Up application uses the following permissions:

- `--share=ipc`: Share IPC namespace with the system
- `--socket=fallback-x11`: Access X11 display (with Wayland fallback)
- `--socket=wayland`: Access Wayland display server
- `--filesystem=/etc/os-release:ro`: Read-only access to OS release info
- `--filesystem=host:reset`: Reset to full host access (privileged operations)

For security audits, all permissions must be clearly documented and justified.

## Maintenance

### Regular Maintenance Tasks

1. **Update Rust Version**: As the application grows, update the GNOME Rust extension version
2. **Archive Old Releases**: Remove old Flatpak bundles from GitHub releases to save space
3. **Update Dependencies**: Run `flatpak-builder --cleanup` to clear old artifacts
4. **Test against multiple platforms**: Consider testing on different distributions

### Cleaning Build Artifacts

```bash
# Remove Flatpak build directory
rm -rf builddir

# Clean re