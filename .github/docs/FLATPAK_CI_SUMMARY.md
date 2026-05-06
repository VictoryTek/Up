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
