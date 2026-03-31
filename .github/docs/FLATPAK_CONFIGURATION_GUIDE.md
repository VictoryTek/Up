# Flatpak Configuration Guide

This document provides step-by-step instructions for configuring the Flatpak CI/CD for first-time setup and ongoing maintenance.

## Initial Setup Checklist

### 1. Install GitHub Actions Dependencies

Ensure your GitHub Actions workflow has access to:
- ✅ Ubuntu 22.04 runner (or compatible)
- ✅ GNOME 46 SDK and Platform
- ✅ Flatpak and flatpak-builder
- ✅ Rust toolchain (via GNOME extension)

### 2. Configure Flatpak Manifest

The ```io.github.up.json``` file defines your Flatpak package:

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
    "finish-args": [
        "--share=ipc",
        "--socket=fallback-x11",
        "--socket=wayland",
        "--talk-name=org.freedesktop.Flatpak",
        "--talk-name=org.freedesktop.PolicyKit1",
        "--filesystem=/etc/os-release:ro",
        "--filesystem=/etc/nixos:ro"
    ],
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

### 3. Configure Build Script

The {}.github/scripts/build-flatpak.sh} script automates local builds:

```bash
#!/usr/bin/env bash
set -euo pipefail

echo "=== Building Up Flatpak Package ==="

# Verify prerequisites
flatpak --version
flatpak-builder --version

# Install GNOME 46 SDK
flatpak install -y flathub org.gnome.Platform//46 org.gnome.Sdk//46

# Install Rust extension
flatpak install -y flathub org.freedesktop.Sdk.Extension.rust-stable//24.08

# Build Flatpak
flatpak-builder --user --install --force-clean builddir io.github.up.json

# Create distributable bundle
flatpak build-bundle --unate builddir/repo up-release.flatpak io.github.up
```

### 4. Configure GitHub Release Process

#### 4.1 Create GitHub Releases Manually (Optional)

```bash
# Tag a release
git tag -a v1.0.0 -m "Release version 1.0.0"

# Push to trigger CI
git push origin v1.0.0

# GitHub Actions automatically builds and attaches Flatpak bundle
```

#### 4.2 Automated Release Workflow

The CI/CD workflow automatically:
- Detects git tags
- Builds Flatpak package
- Uploads to GitHub Releases
- Attaches the `.flatpak` bundle as a release asset

### 5. Managing Dependencies

#### 5.1 Update GNOME SDK Version

To migrate from GNOME 45 to 46:

```bash
# 1. Update io.github.up.json
# Change "runtime-version": "45" to "46"
# Change "sdk-version": "45" to "46"

# 2. Update CI workflow YAML
# Find all references to "45" and update to "46"

# 3. Test locally
./scripts/build-flatpak.sh

# 4. Commit changes
git add io.github.up.json .github/workflows/*.yml
git commit -m "chore: Update to GNOME SDK 46"
git push origin main
```

#### 5.2 Update Flatpak Permissions

To modify the application's sandbox permissions, edit `io.github.up.json`:

```json
{
    "finish-args": [
        "--share=ipc",
        "--socket=wayland",
        "--talk-name=org.freedesktop.Flatpak",
        "--filesystem=home:ro"  // Read-only home access
    ]
}
```

### 6. Local Development Workflow

```bash
# Clone repository
git clone https://github.com/user/up.git
cd up

# Build and run
flatpak-builder --user --install --force-clean builddir io.github.up.json
flatpak run io.github.up
```

### 7. Common Issues and Solutions

#### Issue: "Failed to initialize locks: Unable to create /var/tmp/flatpak-build.lock"

**Solution:**
```bash
# Ensure you have write access to /var/tmp
sudo chown -R $(whoami) /var/tmp
```

#### Issue: "Error: Flatpak not installed"

**Solution:**
```bash
# Install Flatpak and flatpak-builder
sudo apt install flatpak flatpak-builder
```

#### Issue: "Module 'rustup-init' failed to build"

**Solution:**
```bash
# Clean build directory and re-run
rm -rf builddir
flatpak-builder --user --install --force-clean builddir io.github.up.json
```

#### Issue: "GNOME SDK version mismatch"

**Solution:**
```bash
# Uninstall conflicting versions
flatpak uninstall org.gnome.Sdk -y

# Install correct version
flatpak install flathub org.gnome.Sdk//46 -y
```

#### Issue: "Flatpak bundle validation failed"

**Solution:**
```bash
# Run validation tools
flatpak build-bundle builddir/repo test.flatpak io.github.up
flatpak-builder --fail-on-warning --user --install builddir io.github.up.json
```

### 8. Security Best Practices

- **Minimize Permissions**: Only request filesystem access and D-Bus names required for functionality
- **Use Sandboxing**: Keep Flatpak approval requirements to a minimum
- **Review Dependencies**: Regularly audit Cargo dependencies for vulnerabilities
- **Sign Releases**: Use GPG signing for git tags and release assets

### 9. Maintenance Tasks

#### Monthly Tasks

- Review GitHub Actions workflow runs for failures
- Check for GNOME SDK updates
- Audit Flatpak permissions and dependencies

#### Quarterly Tasks

- Review update frequency and cache hit rates
- Evaluate Flatpak build speed and optimization opportunities
- Review security advisories for Rust and GNOME packages

#### Release Tasks

1. Update version in `Cargo.toml`
2. Update `io.github.up.metainfo.xml` with release notes
3. Tag release and push to trigger CI
4. Monitor GitHub Actions build

### 10. Resources and Documentation

- [Flatpak Documentation](https://docs.flatpak.org/)
- [Flatpak Builder Guide](https://docs.flatpak.org/en/latest/flatpak-builder.html)
- [GNOME Circle](https://circle.gnome.org/)
- [Flatpak GitHub Actions Examples](https://github.com/search?q=flatpak+github+actions)
- [Rust GNOME SDK Extension](https://gitlab.gnome.org/GNOME/continuous-extensions)
