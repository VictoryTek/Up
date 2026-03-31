#!/usr/bin/env bash
set -euo pipefail

# Build the Flatpak package for Up
# Run from the repository root directory

echo "=== Building Up Flatpak Package ==="
echo ""

# Check if we're in the right directory
if [ ! -f "io.github.up.json" ]; then
  echo "Error: io.github.up.json not found. Run this script from the repository root."
  exit 1
fi

# Check for flatpak and flatpak-builder
echo "Step 1: Verifying Flatpak tools..."
if ! command -v flatpak &> /dev/null; then
  echo "Error: flatpak not found. Please install Flatpak first."
  echo "  Installation guide: https://flatpak.org/setup/"
  exit 1
fi

if ! command -v flatpak-builder &> /dev/null; then
  echo "Error: flatpak-builder not found. Please install it."
  echo "  Example: sudo apt install flatpak-builder"
  exit 1
fi

flatpak --version
flatpak-builder --version
echo ""

# Ensure GNOME 46 platform and SDK are installed
echo "Step 2: Setting up GNOME 46 SDK..."
flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub
flatpak install -y flathub org.gnome.Platform//46 org.gnome.Sdk//46
echo ""

# Ensure Rust GNOME extension is installed
echo "Step 3: Installing Rust extension..."
flatpak install -y flathub org.freedesktop.Sdk.Extension.rust-stable//24.08
echo ""

# Clean and build
echo "Step 4: Building Flatpak..."
flatpak-builder --user --install --force-clean builddir io.github.up.json
echo ""

# Show the result
echo "Step 5: Flatpak build complete!"
echo ""
echo "Installed applications:"
flatpak list --app | grep -i "up" || true

# Create a bundle for distribution
echo ""
echo "Step 6: Creating Flatpak bundle for distribution..."
flatpak build-bundle --unate builddir/repo up-release.flatpak io.github.up

echo ""
echo "Bundle created: up-release.flatpak"
echo "File size: $(du -h up-release.flatpak | cut -f1)"
echo ""
echo "=== Build Complete ==="
echo ""
echo "To run the app:"
echo "  flatpak run io.github.up"
echo ""
echo "To uninstall:"
echo "  flatpak uninstall io.github.up"
