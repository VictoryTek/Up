import update
import upgrade
import setup
import platform
import os

def detect_distro():
    """
    Detects the Linux distribution name and version.
    Returns a tuple: (distro_name, version)
    """
    if os.name != 'posix':
        return (platform.system(), platform.version())
    try:
        # Try to read /etc/os-release (most modern distros)
        with open('/etc/os-release', 'r') as f:
            lines = f.readlines()
        info = {}
        for line in lines:
            if '=' in line:
                k, v = line.strip().split('=', 1)
                info[k] = v.strip('"')
        name = info.get('NAME', 'Unknown')
        version = info.get('VERSION_ID', info.get('VERSION', 'Unknown'))
        return (name, version)
    except Exception:
        # Fallback to platform module
        return (platform.system(), platform.release())

def main():
    distro, version = detect_distro()
    print(f"Detected distro: {distro} (version: {version})")
    print("Welcome to Up!")
    print("Select an option:")
    print("1. Update system")
    print("2. Upgrade system")
    print("3. Setup system")
    choice = input("Enter your choice (1/2/3): ")

    if choice == '1':
        update.update_system()
    elif choice == '2':
        upgrade.upgrade_system()
    elif choice == '3':
        setup.setup_system()
    else:
        print("Invalid choice.")

if __name__ == "__main__":
    main()