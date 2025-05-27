# Up

<!-- <img src="https://github.com/VictoryTek/Vauxite/blob/main/vauxite.png" /> -->

A linux Utility

Clone the Repo:
```
git clone https://github.com/VictoryTek/Up
```
Change Directory:
```
cd Up
```
Run the CLI version:
```
python up-cli.py
```

---

## Running the GTK GUI (Linux only)

1. **Install dependencies:**
   - On Ubuntu/Debian:
     ```
     sudo apt install python3-gi python3-gi-cairo gir1.2-gtk-4.0
     ```
   - On Fedora:
     ```
     sudo dnf install python3-gobject gtk4
     ```
   - On Arch:
     ```
     sudo pacman -S python-gobject gtk4
     ```
2. **Run the GUI:**
   ```
   python up.py
   ```

**Note:**
- The GUI version is `up.py` and the CLI version is `up-cli.py`.
- The GUI requires Linux with GTK4 and PyGObject. It will not run natively on Windows or macOS.
- For best results, run as a user with sudo privileges for system operations.

---
