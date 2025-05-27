import os
import subprocess

def run_update(distro):
    """Run the appropriate update logic for the detected distro."""
    if not distro:
        print("Could not detect Linux distribution.")
        return
    if distro in ['fedora', 'rhel', 'centos']:
        # Detect if it's an rpm-ostree system (Silverblue/Kinoite/etc)
        if os.path.exists('/usr/bin/rpm-ostree'):
            update_silverblue()
        else:
            update_fedora()
    elif distro == 'bazzite':
        update_bazzite()
    elif distro == 'nobara':
        update_nobara()
    elif distro in ['ubuntu', 'debian']:
        update_ubuntu()
    elif distro in ['arch', 'manjaro']:
        update_arch()
    elif distro in ['opensuse', 'suse']:
        update_opensuse()
    elif distro == 'nixos':
        update_nixos()
    else:
        update_other(distro)

def update_fedora():
    print("Detected Fedora. Updating packages...")
    try:
        subprocess.run(['sudo', 'dnf', 'upgrade', '--refresh', '-y'], check=True)
        print("Fedora packages updated successfully.")
    except subprocess.CalledProcessError as e:
        print(f"Error updating Fedora: {e}")

def update_ubuntu():
    print("Detected Ubuntu/Debian. Updating packages...")
    try:
        subprocess.run(['sudo', 'apt', 'update'], check=True)
        subprocess.run(['sudo', 'apt', 'upgrade', '-y'], check=True)
        print("Ubuntu/Debian packages updated successfully.")
    except subprocess.CalledProcessError as e:
        print(f"Error updating Ubuntu/Debian: {e}")

def update_arch():
    print("Detected Arch/Manjaro. Updating packages...")
    try:
        subprocess.run(['sudo', 'pacman', '-Syu', '--noconfirm'], check=True)
        print("Arch/Manjaro packages updated successfully.")
    except subprocess.CalledProcessError as e:
        print(f"Error updating Arch/Manjaro: {e}")

def update_opensuse():
    print("Detected openSUSE. Updating packages...")
    try:
        subprocess.run(['sudo', 'zypper', 'dup', '-y'], check=True)
        print("openSUSE packages updated successfully.")
    except subprocess.CalledProcessError as e:
        print(f"Error updating openSUSE: {e}")

def update_silverblue():
    print("Detected Fedora Silverblue/Kinoite (rpm-ostree). Updating system...")
    try:
        subprocess.run(['rpm-ostree', 'upgrade'], check=True)
        print("Silverblue/Kinoite system updated successfully.")
    except subprocess.CalledProcessError as e:
        print(f"Error updating Silverblue/Kinoite: {e}")

def update_bazzite():
    print("Detected Bazzite. Updating system with Topgrade...")
    try:
        subprocess.run(['topgrade', '--yes'], check=True)
        print("Bazzite system updated successfully.")
    except subprocess.CalledProcessError as e:
        print(f"Error updating Bazzite: {e}")

def update_nixos():
    print("Detected NixOS. Updating system...")
    try:
        subprocess.run(['sudo', 'nix-channel', '--update'], check=True)
        subprocess.run(['sudo', 'nixos-rebuild', 'switch', '--upgrade'], check=True)
        print("NixOS system updated successfully.")
    except subprocess.CalledProcessError as e:
        print(f"Error updating NixOS: {e}")

def update_nobara():
    print("Detected Nobara. Updating and upgrading with nobara-sync cli...")
    try:
        subprocess.run(['nobara-sync', 'cli'], check=True)
        print("Nobara system updated and upgraded successfully.")
    except subprocess.CalledProcessError as e:
        print(f"Error updating Nobara: {e}")

def update_other(distro):
    print(f"Detected {distro}. No update logic implemented.")
