#!/usr/bin/env bash
set -euo pipefail

# Verification script for Flatpak build prerequisites
# Run this before building to ensure all dependencies are installed

echo "=== Flatpak Build Environment Verification ==="
echo ""

# Check if running on Linux
if [ "$(uname)" != "Linux" ]; then
    echo "⚠️  Warning: Flatpak is primarily supported on Linux."
    echo "   Some features may not work on other operating systems."
    echo ""
fi

# Step 1: Check flatpak installation
echo "Step 1: Verifying Flatpak installation..."
if ! command -v flatpak &> /dev/null; then
    echo "❌ Error: flatpak not found!"
    echo "   Installation guide: https://flatpak.org/setup/"
    exit 1
else
    FLATPAK_VERSION=$(flatpak --version | awk '{print $2}')
    echo "✅ flatpak v$FLATPAK_VERSION installed"
fi
echo ""

# Step 2: Check flatpak-builder installation
echo "Step 2: Verifying flatpak-builder installation..."
if ! command -v flatpak-builder &> /dev/null; then
    echo "❌ Error: flatpak-builder not found!"
    echo "   Installation guide: https://globalexploits.com/tools/flatpak-builder/"
    exit 1
else
    BUILDER_VERSION=$(flatpak-builder --version | awk '{print $2}')
    echo "✅ flatpak-builder v$BUILDER_VERSION installed"
fi
echo ""

# Step 3: Check Flathub remote
echo "Step 3: Verifying Flathub remote configuration..."
if flatpak remote-list | grep -q flathub; then
    echo "✅ Flathub remote configured"
else
    echo "⚠️  Flathub remote not configured. Adding..."
    flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub
    echo "✅ Flathub remote added"
fi
echo ""

# Step 4: Check GNOME 46 Platform
echo "Step 4: Verifying GNOME 46 Platform installation..."
if flatpak install --show-details org.gnome.Platform//46 &> /dev/null; then
    echo "✅ org.gnome.Platform//46 installed"
else
    echo "❌ org.gnome.Platform//46 not installed. Installing..."
    flatpak install -y flathub org.gnome.Platform//46
    echo "✅ org.gnome.Platform//46 installed"
fi
echo ""

# Step 5: Check GNOME 46 SDK
echo "Step 5: Verifying GNOME 46 SDK installation..."
if flatpak install --show-details org.gnome.Sdk//46 &> /dev/null; then
    echo "✅ org.gnome.Sdk//46 installed"
else
    echo "❌ org.gnome.Sdk//46 not installed. Installing..."
    flatpak install -y flathub org.gnome.Sdk//46
    echo "✅ org.gnome.Sdk//46 installed"
fi
echo ""

# Step 6: Check Rust GNOME extension
echo "Step 6: Verifying Rust GNOME extension installation..."
if flatpak install --show-details org.freedesktop.Sdk.Extension.rust-stable//24.08 &> /dev/null; then
    echo "✅ org.freedesktop.Sdk.Extension.rust-stable//24.08 installed"
else
    echo "❌ org.freedesktop.Sdk.Extension.rust-stable//24.08 not installed. Installing..."
    flatpak install -y flathub org.freedesktop.Sdk.Extension.rust-stable//24.08
    echo "✅ org.freedesktop.Sdk.Extension.rust-stable//24.08 installed"
fi
echo ""

# Step 7: Check required tools (desktop-file-validate, appstreamcli)
echo "Step 7: Checking additional validation tools..."

# Desktop file validation
if command -v desktop-file-validate &> /dev/null; then
    echo "✅ desktop-file-validate installed"
else
    echo "⚠️  desktop-file-validate not installed (optional)"
    echo "   Install with: sudo apt install desktop-file-utils (Debian/Ubuntu)"
    echo "                  sudo dnf install desktop-file-utils (Fedora)"
fi

# AppStream validation
if command -v appstreamcli &> /dev/null; then
    echo "✅ appstreamcli installed"
else
    echo "⚠️  appstreamcli not installed (optional)"
    echo "   Install with: sudo apt install appstream (Debian/Ubuntu)"
    echo "                  sudo dnf install appstream (Fedora)"
fi
echo ""

# Step 8: Check disk space
echo "Step 8: Verifying available disk space..."
DISK_USAGE=$(df -h . | awk 'NR==2 {print $4}')
echo "✅ Available disk space: $DISK_USAGE in repository directory"
echo ""

# Step 9: Check repository root
echo "Step 9: Verifying repository structure..."
if [ ! -f "io.github.up.json" ]; then
    echo "❌ Error: io.github.up.json not found"
    echo "   Run this script from the repository root directory"
    exit 1
fi

if [ ! -f "Cargo.toml" ]; then
    echo "❌ Error: Cargo.toml not found"
    echo "   Run this script from the repository root directory"
    exit 1
fi

if [ ! -f "meson.build" ]; then
    echo "❌ Error: meson.build not found"
    echo "   Run this script from the repository root directory"
    exit 1
fi

echo "✅ Repository structure verified"
echo ""

# Final summary
echo "=== Verification Summary ==="
echo ""
echo "All prerequisites for Flatpak build are satisfied!"
echo ""
echo "Next steps:"
echo "  1. Run ./scripts/build-flatpak.sh to build the application"
echo "  2. Run flatpak run io.github.up to launch the application"
echo ""
