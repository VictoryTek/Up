import subprocess
import up

def upgrade_system():
    """
    Upgrades the system packages using the appropriate package manager.
    Supports apt (Debian/Ubuntu), dnf (Fedora), and NixOS.
    For NixOS, prompts for a target version and upgrades to that version by setting the nixos channel and rebuilding.
    """
    distro, _ = up.detect_distro()
    try:
        print("Upgrading system packages...")
        if "NixOS" in distro:
            # Ask user for the target version
            target_version = input("Enter the NixOS version to upgrade to (e.g., 25.05): ").strip()
            if not target_version:
                print("No version entered. Aborting upgrade.")
                return
            # Set the nixos channel to the target version
            subprocess.run(["sudo", "nix-channel", "--add", f"https://channels.nixos.org/nixos-{target_version}", "nixos"], check=True)
            subprocess.run(["sudo", "nix-channel", "--update"], check=True)
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
