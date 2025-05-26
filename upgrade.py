import subprocess
import up

def upgrade_system():
    """
    Upgrades the system packages using the appropriate package manager.
    Supports apt (Debian/Ubuntu), dnf (Fedora), and NixOS.
    For NixOS, this will perform a version upgrade (nixos-rebuild switch --upgrade).
    """
    distro, _ = up.detect_distro()
    try:
        print("Upgrading system packages...")
        if "NixOS" in distro:
            # NixOS: version upgrade
            subprocess.run(["sudo", "nixos-rebuild", "switch", "--upgrade"], check=True)
        elif "Fedora" in distro or "Red Hat" in distro or "CentOS" in distro:
            subprocess.run(["sudo", "dnf", "upgrade", "--refresh", "-y"], check=True)
        else:
            subprocess.run(["sudo", "apt", "upgrade", "-y"], check=True)
        print("System upgrade complete.")
    except subprocess.CalledProcessError as e:
        print(f"Upgrade failed: {e}")

if __name__ == "__main__":
    upgrade_system()
