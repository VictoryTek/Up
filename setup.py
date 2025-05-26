import subprocess
import up

def setup_system():
    """
    Sets up the system with initial configuration and essential packages.
    Supports apt (Debian/Ubuntu), dnf (Fedora), and NixOS.
    """
    distro, _ = up.detect_distro()
    try:
        print("Updating package list and installing essential packages...")
        if "NixOS" in distro:
            # NixOS: install packages for the current user
            essential_packages = ["curl", "git", "vim"]
            subprocess.run(["nix-env", "-iA"] + [f"nixpkgs.{pkg}" for pkg in essential_packages], check=True)
        elif "Fedora" in distro or "Red Hat" in distro or "CentOS" in distro:
            subprocess.run(["sudo", "dnf", "makecache"], check=True)
            essential_packages = ["curl", "git", "vim"]
            subprocess.run(["sudo", "dnf", "install", "-y"] + essential_packages, check=True)
        else:
            subprocess.run(["sudo", "apt", "update"], check=True)
            essential_packages = ["curl", "git", "vim"]
            subprocess.run(["sudo", "apt", "install", "-y"] + essential_packages, check=True)
        print("System setup complete.")
    except subprocess.CalledProcessError as e:
        print(f"Setup failed: {e}")

if __name__ == "__main__":
    setup_system()
