import subprocess
import up

def update_system():
    """
    Updates the system package list using the appropriate package manager.
    Supports apt (Debian/Ubuntu), dnf (Fedora), and NixOS.
    For NixOS, this will update channels and rebuild the system (like a normal upgrade).
    """
    distro, _ = up.detect_distro()
    try:
        print("Updating package list...")
        if "NixOS" in distro:
            # NixOS: update channels and rebuild system
            subprocess.run(["sudo", "nix-channel", "--update"], check=True)
            subprocess.run(["sudo", "nixos-rebuild", "switch"], check=True)
        elif "Fedora" in distro or "Red Hat" in distro or "CentOS" in distro:
            subprocess.run(["sudo", "dnf", "check-update"], check=True)
        else:
            subprocess.run(["sudo", "apt", "update"], check=True)
        print("Package list update complete.")
    except subprocess.CalledProcessError as e:
        print(f"Update failed: {e}")

if __name__ == "__main__":
    update_system()
