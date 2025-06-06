import os
import subprocess

def detect_distro():
    """Detect the Linux distribution using /etc/os-release."""
    distro = None
    if os.path.exists('/etc/os-release'):
        with open('/etc/os-release') as f:
            for line in f:
                if line.startswith('ID='):
                    distro = line.strip().split('=')[1].strip('"')
                    break
    return distro

def run_upgrade(distro, version=None):
    """Run the appropriate upgrade logic for the detected distro."""
    if not distro:
        print("Could not detect Linux distribution.")
        return
    if distro in ['fedora', 'rhel', 'centos']:
        # Detect if it's an rpm-ostree system (Silverblue/Kinoite/etc)
        if os.path.exists('/usr/bin/rpm-ostree'):
            upgrade_silverblue(version)
        else:
            upgrade_fedora()
    elif distro == 'bazzite':
        upgrade_bazzite()
    elif distro in ['ubuntu', 'debian']:
        upgrade_ubuntu()
    elif distro in ['arch', 'manjaro']:
        upgrade_arch()
    elif distro in ['opensuse', 'suse']:
        upgrade_opensuse()
    elif distro == 'nixos':
        upgrade_nixos(version)
    else:
        upgrade_other(distro)

def upgrade_fedora():
    print("Detected Fedora. Running system upgrade...")
    try:
        subprocess.run(['sudo', 'dnf', 'upgrade', '--refresh', '-y'], check=True)
        print("Fedora system upgraded to latest packages. For major version upgrades, use 'dnf system-upgrade'.")
    except subprocess.CalledProcessError as e:
        print(f"Error upgrading Fedora: {e}")

def upgrade_ubuntu():
    print("Detected Ubuntu. Running system upgrade...")
    try:
        subprocess.run(['sudo', 'apt', 'update'], check=True)
        subprocess.run(['sudo', 'apt', 'upgrade', '-y'], check=True)
        subprocess.run(['sudo', 'do-release-upgrade', '-f', 'DistUpgradeViewNonInteractive'], check=True)
        print("Ubuntu upgraded to latest release.")
    except subprocess.CalledProcessError as e:
        print(f"Error upgrading Ubuntu: {e}")

def upgrade_arch():
    print("Detected Arch Linux. Running system upgrade...")
    try:
        subprocess.run(['sudo', 'pacman', '-Syu', '--noconfirm'], check=True)
        print("Arch Linux upgraded to latest packages.")
    except subprocess.CalledProcessError as e:
        print(f"Error upgrading Arch Linux: {e}")

def upgrade_opensuse():
    print("Detected openSUSE. Running system upgrade...")
    try:
        subprocess.run(['sudo', 'zypper', 'dup', '-y'], check=True)
        print("openSUSE upgraded to latest release.")
    except subprocess.CalledProcessError as e:
        print(f"Error upgrading openSUSE: {e}")

def upgrade_silverblue(version=None):
    print("Detected Fedora Silverblue/Kinoite (rpm-ostree). Running system version upgrade...")
    # Detect current ref for template
    try:
        current_ref = subprocess.check_output(['rpm-ostree', 'status', '--json'], text=True)
        import json
        ref = json.loads(current_ref)["deployments"][0]["base-commit-meta"]["ostree.ref"]
    except Exception:
        # Fallback to default template if detection fails
        ref = "fedora:fedora/40/x86_64/silverblue"
    # Extract base ref up to version number
    import re
    match = re.match(r"(.*/)(\d+)(/.*)", ref)
    if match:
        base, cur_ver, tail = match.groups()
        print(f"Current ref: {ref}")
        if version is None:
            print(f"To rebase, enter the new version number you wish to rebase to (e.g., 42):")
            version = input("Enter new version number (or leave blank to skip): ").strip()
        if version:
            new_ref = f"{base}{version}{tail}"
            try:
                result = subprocess.run(['rpm-ostree', 'rebase', new_ref], check=True, text=True, capture_output=True)
                print(result.stdout)
                print(f"Rebased to {new_ref} successfully.")
            except subprocess.CalledProcessError as e:
                print(e.stdout if hasattr(e, 'stdout') and e.stdout else f"Error rebasing Silverblue/Kinoite: {e}")
        else:
            print("No version entered. Skipping rebase.")
    else:
        print("Could not parse current ref. Please enter the full ref as before.")
        if version is None:
            new_ref = input("Enter new rpm-ostree ref (or leave blank to skip): ").strip()
        else:
            new_ref = version
        if new_ref:
            try:
                result = subprocess.run(['rpm-ostree', 'rebase', new_ref], check=True, text=True, capture_output=True)
                print(result.stdout)
                print(f"Rebased to {new_ref} successfully.")
            except subprocess.CalledProcessError as e:
                print(e.stdout if hasattr(e, 'stdout') and e.stdout else f"Error rebasing Silverblue/Kinoite: {e}")
        else:
            print("No ref entered. Skipping rebase.")

def upgrade_bazzite():
    print("Detected Bazzite. Running system version upgrade with Topgrade...")
    try:
        subprocess.run(['topgrade', '--yes'], check=True)
        print("Bazzite system upgraded successfully.")
    except subprocess.CalledProcessError as e:
        print(f"Error upgrading Bazzite: {e}")

def upgrade_nixos(version=None):
    print("Detected NixOS. Running system version upgrade...")
    if version is None:
        version = input("Enter the NixOS version/channel to upgrade to (e.g., nixos-24.05): ").strip()
    if not version:
        print("No version entered. Aborting NixOS upgrade.")
        return
    try:
        subprocess.run(['sudo', 'nix-channel', '--add', f'https://nixos.org/channels/{version}', 'nixos'], check=True)
        subprocess.run(['sudo', 'nix-channel', '--update'], check=True)
        subprocess.run(['sudo', 'nixos-rebuild', 'switch', '--upgrade'], check=True)
        print(f"NixOS upgraded to {version} successfully.")
    except subprocess.CalledProcessError as e:
        print(f"Error upgrading NixOS: {e}")

def upgrade_other(distro):
    print(f"Detected {distro}. No upgrade logic implemented.")

def main():
    distro = detect_distro()
    run_upgrade(distro)

if __name__ == "__main__":
    main()
