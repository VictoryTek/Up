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
| Nixos  | ✅ NIX | ❌ (Declarative)
| Ubuntu / Debian | ✅ APT | ✅ do-release-upgrade |
| Fedora | ✅ DNF | ✅ dnf system-upgrade |
| Arch Linux | ✅ Pacman | ❌ (rolling release) |
| openSUSE Leap | ✅ Zypper | ✅ zypper dup |

## Installation

### From Nix Flake

```bash
# Run directly
nix run github:victorytek/up

# Install to profile
nix profile add github:victorytek/up

# Development shell
nix develop github:victorytek/up
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
git clone https://github.com/victorytek/up.git
cd up

# Build and run
cargo run

# With Nix
nix develop
cargo run
```

## CI/CD

The project uses GitHub Actions for continuous integration:

- **Build Testing**: Runs `cargo fmt`, `cargo clippy`, `cargo build`, and `cargo test` on all pull requests and pushes to `main`
- **Validation**: Validates the desktop file and AppStream metadata on every run

Automated release assets are planned for a future release.

Up is built to run correctly inside a Flatpak sandbox (host commands are routed
via `flatpak-spawn --host`), so a local `flatpak-builder` build works, but there
is no official Flathub distribution.

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

