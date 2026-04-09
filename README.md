# Up

A modern Linux system update & upgrade application built with Rust, GTK4, and libadwaita.

![GNOME](https://img.shields.io/badge/Desktop-GNOME-blue)
![Rust](https://img.shields.io/badge/Language-Rust-orange)
![License](https://img.shields.io/badge/License-GPL--3.0-green)

## Features

### Update Mode
- **Auto-detects** your OS package manager (APT, DNF, Pacman, Zypper)
- **Flatpak** — updates all installed Flatpak apps
- **Homebrew** — updates Linuxbrew packages (if installed)
- **Nix** — updates Nix profile packages (flake and legacy)
- **One-click "Update All"** with live terminal output dropdown

### Upgrade Mode
- Upgrade your distro to the **next major version** (Ubuntu, Fedora, openSUSE)
- **Prerequisite checks:** verifies packages are current, disk space is sufficient
- **Backup confirmation** before proceeding
- **Guided flow** with confirmation dialog before execution
- Live streaming output during the upgrade process

## Supported Distributions

| Distro | Update | Upgrade |
|--------|--------|---------|
| Ubuntu / Debian | ✅ APT | ✅ do-release-upgrade |
| Fedora | ✅ DNF | ✅ dnf system-upgrade |
| Arch Linux | ✅ Pacman | ❌ (rolling release) |
| openSUSE Leap | ✅ Zypper | ✅ zypper dup |

## Installation

### From Nix Flake

```bash
# Run directly
nix run github:user/up

# Install to profile
nix profile install github:user/up

# Development shell
nix develop github:user/up
```

### From Flatpak (local build)

```bash
# Clone the repository
git clone https://github.com/user/up.git
cd up

# Install GNOME SDK
flatpak install org.gnome.Sdk//46 org.gnome.Platform//46
flatpak install org.freedesktop.Sdk.Extension.rust-stable//24.08

# Build and install (run from the project root)
flatpak-builder --user --install --force-clean builddir io.github.up.json

# Or use the convenience script
./scripts/build-flatpak.sh
```

### From Source

```bash
# Dependencies (Fedora example)
sudo dnf install gtk4-devel libadwaita-devel gcc pkg-config meson ninja-build

# Build
cargo build --release

# Or with meson
meson setup builddir --buildtype=release
meson compile -C builddir
meson install -C builddir
```

## Development

```bash
# Clone
git clone https://github.com/user/up.git
cd up

# Build and run
cargo run

# With Nix
nix develop
cargo run

# Build Flatpak locally
./scripts/build-flatpak.sh
```

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

## Architecture

```
src/
├── main.rs              # Entry point
├── app.rs               # GtkApplication setup
├── runner.rs            # Command execution with streaming output
├── upgrade.rs           # Distro upgrade logic & prerequisite checks
├── ui/
│   ├── mod.rs
│   ├── window.rs        # Main application window
│   ├── update_row.rs    # Per-backend status row widget
│   ├── log_panel.rs     # Expandable terminal output panel
│   └── upgrade_page.rs  # Upgrade mode UI with guided flow
├── backends/
│   ├── mod.rs           # Backend trait & detection
│   ├── os_package_manager.rs  # APT, DNF, Pacman, Zypper
│   ├── flatpak.rs
│   ├── homebrew.rs
│   └── nix.rs
```

## License

GPL-3.0-or-later

<!-- <img src="https://github.com/VictoryTek/Vauxite/blob/main/vauxite.png" /> -->

A linux Utility

# Flatpak Build (Premium Release Process)

The application is packaged as a Flatpak for universal Linux compatibility.

## Automated Release with GitHub Actions

When a Git tag is pushed, GitHub Actions automatically:
- Builds and tests the application
- Creates a Flatpak bundle
- Publishes it as a GitHub Release asset

Example:
```bash
# Tag a release
git tag -a v1.0.0 -m "Release version 1.0.0"
git push origin v1.0.0

# GitHub Actions will automatically build and release the Flatpak
```

## Manual Flatpak Build

You can build the Flatpak manually:

```bash
# Clone the repository
git clone https://github.com/user/up.git
cd up

# Build and install the Flatpak package
./scripts/build-flatpak.sh

# Run the application
flatpak run io.github.up
```

## System Requirements

Flatpak packaging requires:
- Flatpak and flatpak-builder
- GNOME 46 SDK and Platform
- Rust 1.75+ (provided via GNOME extension)

See [.github/docs/FLATPAK_README.md](.github/docs/FLATPAK_README.md) for comprehensive documentation.

