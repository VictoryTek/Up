# Standalone CLI menu for system maintenance
import os
import importlib.util
import logging
from datetime import datetime

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

def load_module(module_name):
    module_file = os.path.join(os.path.dirname(__file__), f"{module_name}.py")
    if not os.path.exists(module_file):
        return None
    spec = importlib.util.spec_from_file_location(module_name, module_file)
    if spec is None:
        return None
    module = importlib.util.module_from_spec(spec)
    try:
        spec.loader.exec_module(module)
        return module
    except Exception as e:
        print(f"Error importing {module_name}: {e}")
        return None

distro = detect_distro()

update_module = load_module('update')
upgrade_module = load_module('upgrade')
setup_module = load_module('setup')
# You can add similar lines for other modules when you create them.

# Save generated files to the parent 'Up' folder
PARENT_DIR = os.path.dirname(os.path.abspath(__file__))
LOG_FILE = os.path.join(PARENT_DIR, 'maintenance.log')
LAST_ACTION_FILE = os.path.join(PARENT_DIR, 'last_action.txt')

def save_last_action(action):
    try:
        with open(LAST_ACTION_FILE, 'w') as f:
            f.write(action)
    except Exception as e:
        print(f"Error saving last action: {e}")

def load_last_action():
    if os.path.exists(LAST_ACTION_FILE):
        try:
            with open(LAST_ACTION_FILE, 'r') as f:
                return f.read().strip()
        except Exception as e:
            print(f"Error loading last action: {e}")
    return None

# Setup logging
logging.basicConfig(
    filename=LOG_FILE,
    level=logging.INFO,
    format='%(asctime)s [%(levelname)s] %(message)s',
    datefmt='%Y-%m-%d %H:%M:%S'
)

def log_action(action, status, error=None):
    msg = f"{action} - {status}"
    if error:
        msg += f" | Error: {error}"
    if status == 'ERROR':
        logging.error(msg)
    else:
        logging.info(msg)

def update():
    print("Running update...")
    print(f"Detected distro: {distro}")
    save_last_action("Update")
    log_action("Update", "START")
    # Ask user if they want to restart after update completes
    while True:
        restart_choice = input("Do you want to restart after the update? (Y/N): ").strip().lower()
        if restart_choice in ('y', 'n'):
            break
        else:
            print("Invalid input. Please enter Y or N.")
    try:
        if update_module and hasattr(update_module, 'run_update'):
            update_module.run_update(distro)
            log_action("Update", "SUCCESS")
        else:
            print("[Placeholder] Update logic not implemented.")
            log_action("Update", "ERROR", "Update logic not implemented.")
    except Exception as e:
        print(f"Error during update: {e}")
        log_action("Update", "ERROR", str(e))
    if restart_choice == 'y':
        restart()

def upgrade():
    print("Running upgrade...")
    print(f"Detected distro: {distro}")
    save_last_action("Upgrade")
    log_action("Upgrade", "START")
    # Ask user if they want to restart after upgrade completes
    while True:
        restart_choice = input("Do you want to restart after the upgrade? (Y/N): ").strip().lower()
        if restart_choice in ('y', 'n'):
            break
        else:
            print("Invalid input. Please enter Y or N.")
    try:
        if upgrade_module and hasattr(upgrade_module, 'run_upgrade'):
            upgrade_module.run_upgrade(distro)
            log_action("Upgrade", "SUCCESS")
        else:
            print("[Placeholder] Upgrade logic not implemented.")
            log_action("Upgrade", "ERROR", "Upgrade logic not implemented.")
    except Exception as e:
        print(f"Error during upgrade: {e}")
        log_action("Upgrade", "ERROR", str(e))
    if restart_choice == 'y':
        restart()

def setup():
    print("Running setup...")
    print(f"Detected distro: {distro}")
    save_last_action("Setup")
    log_action("Setup", "START")
    try:
        if setup_module and hasattr(setup_module, 'run_setup'):
            setup_module.run_setup(distro)
            log_action("Setup", "SUCCESS")
        else:
            print("[Placeholder] Setup logic not implemented.")
            log_action("Setup", "ERROR", "Setup logic not implemented.")
    except Exception as e:
        print(f"Error during setup: {e}")
        log_action("Setup", "ERROR", str(e))

def restart():
    print("Restarting system...")
    log_action("Restart", "START")
    try:
        if os.name == 'nt':
            os.system('shutdown /r /t 0')
        else:
            os.system('sudo reboot')
        log_action("Restart", "SUCCESS")
    except Exception as e:
        print(f"Error during restart: {e}")
        log_action("Restart", "ERROR", str(e))

def main_menu():
    while True:
        last_action = load_last_action()
        print("\n=== Maintenance Menu ===")
        if last_action:
            print(f"Last action: {last_action}")
        print("1. Update")
        print("2. Upgrade")
        print("3. Setup")
        print("4. Restart")
        print("5. Exit")
        choice = input("Select an option (1-5): ").strip()
        if choice == '1':
            update()
        elif choice == '2':
            upgrade()
        elif choice == '3':
            setup()
        elif choice == '4':
            restart()
            break
        elif choice == '5':
            print("Exiting...")
            break
        else:
            print("Invalid choice. Please select a valid option.")

if __name__ == "__main__":
    # If this is the GUI version, launch the GTK GUI
    import gui
    gui.main()
