#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
Up - Universal Linux Maintenance Application
GTK4 GUI for Up Application
Follows Context7 best practices
"""

import sys
import os
import gi
import threading
import platform
from datetime import datetime

# Ensure we're using GTK4
try:
    gi.require_version('Gtk', '4.0')
    from gi.repository import Gtk, Gio, GLib
except Exception as e:
    print("PyGObject with GTK4 is required. Please install it via your package manager.")
    sys.exit(1)

# --- Helper functions ---
def detect_distro():
    if os.path.exists('/etc/os-release'):
        with open('/etc/os-release') as f:
            for line in f:
                if line.startswith('ID='):
                    return line.strip().split('=')[1].strip('"')
    return None

def get_kernel():
    return platform.release()

# --- Progress Dialog ---
class ProgressDialog(Gtk.Window):
    def __init__(self, parent, title, message):
        super().__init__()
        self.set_title(title)
        self.set_transient_for(parent)
        self.set_modal(True)
        self.set_default_size(400, 180)
        self.set_resizable(False)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        box.set_margin_top(18)
        box.set_margin_bottom(18)
        box.set_margin_start(18)
        box.set_margin_end(18)
        self.set_child(box)
        self.label = Gtk.Label(label=message)
        self.label.set_wrap(True)
        box.append(self.label)
        self.progress = Gtk.ProgressBar()
        self.progress.set_show_text(True)
        box.append(self.progress)
        self.output = Gtk.TextView()
        self.output.set_editable(False)
        self.output.set_monospace(True)
        scrolled = Gtk.ScrolledWindow()
        scrolled.set_min_content_height(80)
        scrolled.set_child(self.output)
        box.append(scrolled)
        self.cancel_btn = Gtk.Button(label="Cancel")
        self.cancel_btn.connect("clicked", self.on_cancel)
        box.append(self.cancel_btn)
        self.cancelled = False
        self.pulse_timer = GLib.timeout_add(100, self.pulse)
    def pulse(self):
        if not self.cancelled:
            self.progress.pulse()
            return True
        return False
    def append_output(self, text):
        buf = self.output.get_buffer()
        end = buf.get_end_iter()
        buf.insert(end, text + "\n")
        mark = buf.get_insert()
        self.output.scroll_to_mark(mark, 0.0, False, 0.0, 0.0)
    def update_message(self, msg):
        GLib.idle_add(self.label.set_text, msg)
    def finish(self, success, msg=""):
        self.cancelled = True
        GLib.idle_add(self.progress.set_fraction, 1.0 if success else 0.0)
        GLib.idle_add(self.progress.set_text, "Complete" if success else "Failed")
        if msg:
            self.append_output(msg)
        GLib.idle_add(self.cancel_btn.set_label, "Close")
    def on_cancel(self, btn):
        self.cancelled = True
        self.close()

# --- Log Viewer ---
class LogWindow(Gtk.Window):
    def __init__(self, parent):
        super().__init__()
        self.set_title("Up Logs")
        self.set_transient_for(parent)
        self.set_default_size(700, 500)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        self.set_child(box)
        header = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=12)
        header.set_margin_top(12)
        header.set_margin_start(12)
        header.set_margin_end(12)
        box.append(header)
        title = Gtk.Label(label="<b>Up Logs</b>", use_markup=True)
        title.set_hexpand(True)
        title.set_halign(Gtk.Align.START)
        header.append(title)
        refresh = Gtk.Button(label="Refresh")
        refresh.connect("clicked", self.load_logs)
        header.append(refresh)
        scrolled = Gtk.ScrolledWindow()
        scrolled.set_margin_top(6)
        scrolled.set_margin_bottom(12)
        scrolled.set_margin_start(12)
        scrolled.set_margin_end(12)
        box.append(scrolled)
        self.text = Gtk.TextView()
        self.text.set_editable(False)
        self.text.set_monospace(True)
        self.buf = self.text.get_buffer()
        scrolled.set_child(self.text)
        self.load_logs()
    def load_logs(self, btn=None):
        try:
            log_file = os.path.join(os.path.dirname(__file__), 'maintenance.log')
            if os.path.exists(log_file):
                with open(log_file, 'r') as f:
                    lines = f.readlines()
                display = ''.join(lines[-100:]) if len(lines) > 100 else ''.join(lines)
                self.buf.set_text(display)
                end = self.buf.get_end_iter()
                self.buf.place_cursor(end)
                self.text.scroll_to_mark(self.buf.get_insert(), 0.0, False, 0.0, 0.0)
            else:
                self.buf.set_text("No log file found.")
        except Exception as e:
            self.buf.set_text(f"Error loading logs: {e}")

# --- Main Window ---
class UpWindow(Gtk.ApplicationWindow):
    def __init__(self, app):
        super().__init__(application=app)
        self.set_title("Up")
        self.set_default_size(600, 500)
        self.set_resizable(True)
        self.distro = detect_distro()
        self.create_header()
        self.create_content()
        self.update_distro_info()
    def create_header(self):
        header = Gtk.HeaderBar()
        header.set_title_widget(Gtk.Label(label="Up"))
        self.set_titlebar(header)
        menu_btn = Gtk.MenuButton()
        menu_btn.set_icon_name("open-menu-symbolic")
        header.pack_end(menu_btn)
        menu = Gio.Menu()
        menu.append("About", "app.about")
        menu.append("Logs", "app.logs")
        menu.append("Quit", "app.quit")
        menu_btn.set_menu_model(menu)
        refresh_btn = Gtk.Button()
        refresh_btn.set_icon_name("view-refresh-symbolic")
        refresh_btn.set_tooltip_text("Refresh system information")
        refresh_btn.connect("clicked", self.on_refresh)
        header.pack_end(refresh_btn)
    def create_content(self):
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        box.set_margin_top(24)
        box.set_margin_bottom(24)
        box.set_margin_start(24)
        box.set_margin_end(24)
        self.set_child(box)
        # System Info
        frame = Gtk.Frame(label="System Information")
        box.append(frame)
        info = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        info.set_margin_top(12)
        info.set_margin_bottom(12)
        info.set_margin_start(12)
        info.set_margin_end(12)
        frame.set_child(info)
        self.distro_label = Gtk.Label()
        self.distro_label.set_halign(Gtk.Align.START)
        info.append(self.distro_label)
        kernel_label = Gtk.Label(label=f"Kernel: {get_kernel()}")
        kernel_label.set_halign(Gtk.Align.START)
        info.append(kernel_label)
        # Actions
        frame2 = Gtk.Frame(label="Actions")
        box.append(frame2)
        grid = Gtk.Grid()
        grid.set_row_spacing(12)
        grid.set_column_spacing(12)
        grid.set_margin_top(12)
        grid.set_margin_bottom(12)
        grid.set_margin_start(12)
        grid.set_margin_end(12)
        frame2.set_child(grid)
        # Update
        update_btn = Gtk.Button()
        update_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        update_icon = Gtk.Image.new_from_icon_name("software-update-available-symbolic")
        update_icon.set_pixel_size(32)
        update_label = Gtk.Label(label="<b>Update</b>\nRefresh package lists and install updates", use_markup=True)
        update_box.append(update_icon)
        update_box.append(update_label)
        update_btn.set_child(update_box)
        update_btn.connect("clicked", self.on_update)
        grid.attach(update_btn, 0, 0, 1, 1)
        # Upgrade
        upgrade_btn = Gtk.Button()
        upgrade_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        upgrade_icon = Gtk.Image.new_from_icon_name("system-software-update-symbolic")
        upgrade_icon.set_pixel_size(32)
        upgrade_label = Gtk.Label(label="<b>Upgrade</b>\nUpgrade to newer distribution version", use_markup=True)
        upgrade_box.append(upgrade_icon)
        upgrade_box.append(upgrade_label)
        upgrade_btn.set_child(upgrade_box)
        upgrade_btn.connect("clicked", self.on_upgrade)
        grid.attach(upgrade_btn, 1, 0, 1, 1)
        # Setup
        setup_btn = Gtk.Button()
        setup_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        setup_icon = Gtk.Image.new_from_icon_name("preferences-system-symbolic")
        setup_icon.set_pixel_size(32)
        setup_label = Gtk.Label(label="<b>Setup</b>\nConfigure system settings", use_markup=True)
        setup_box.append(setup_icon)
        setup_box.append(setup_label)
        setup_btn.set_child(setup_box)
        setup_btn.connect("clicked", self.on_setup)
        grid.attach(setup_btn, 0, 1, 1, 1)
        # Restart
        restart_btn = Gtk.Button()
        restart_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        restart_icon = Gtk.Image.new_from_icon_name("system-restart-symbolic")
        restart_icon.set_pixel_size(32)
        restart_label = Gtk.Label(label="<b>Restart</b>\nRestart the system", use_markup=True)
        restart_box.append(restart_icon)
        restart_box.append(restart_label)
        restart_btn.set_child(restart_box)
        restart_btn.connect("clicked", self.on_restart)
        grid.attach(restart_btn, 1, 1, 1, 1)
        for btn in [update_btn, upgrade_btn, setup_btn, restart_btn]:
            btn.set_hexpand(True)
            btn.set_vexpand(True)
            btn.set_size_request(200, 120)
        # Options
        frame3 = Gtk.Frame(label="Options")
        box.append(frame3)
        opts = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        opts.set_margin_top(12)
        opts.set_margin_bottom(12)
        opts.set_margin_start(12)
        opts.set_margin_end(12)
        frame3.set_child(opts)
        self.auto_restart = Gtk.CheckButton(label="Automatically restart after update/upgrade")
        opts.append(self.auto_restart)
        self.verbose = Gtk.CheckButton(label="Show verbose output")
        opts.append(self.verbose)
        # Add View Log button
        view_log_btn = Gtk.Button(label="View Log")
        view_log_btn.connect("clicked", self.on_view_log_clicked)
        opts.append(view_log_btn)

    def on_view_log_clicked(self, btn):
        logwin = LogWindow(self)
        logwin.present()

    def update_distro_info(self):
        if self.distro:
            text = f"Distribution: {self.distro.title()}"
            if self.distro in ['fedora', 'rhel', 'centos'] and os.path.exists('/usr/bin/rpm-ostree'):
                text += " (rpm-ostree system)"
            elif self.distro == 'bazzite':
                text += " (using Topgrade)"
            elif self.distro == 'nobara':
                text += " (using nobara-sync)"
        else:
            text = "Distribution: Unknown"
        self.distro_label.set_text(text)
    def on_refresh(self, btn):
        self.distro = detect_distro()
        self.update_distro_info()
    def run_in_thread(self, op_name, func):
        progress = ProgressDialog(self, f"{op_name} in Progress", f"Running {op_name.lower()} operation...")
        progress.present()
        def run():
            try:
                progress.update_message(f"Starting {op_name.lower()}...")
                progress.append_output(f"Distribution: {self.distro}")
                progress.append_output(f"Operation: {op_name}")
                progress.append_output("=" * 50)
                # Monkey patch print to capture output
                import builtins
                orig_print = print
                def progress_print(*args, **kwargs):
                    msg = ' '.join(str(a) for a in args)
                    progress.append_output(msg)
                    orig_print(*args, **kwargs)
                builtins.print = progress_print
                try:
                    func()
                    success = True
                except Exception as e:
                    progress.append_output(f"Error: {e}")
                    success = False
                finally:
                    builtins.print = orig_print
                progress.finish(success, f"{op_name} completed." if success else f"{op_name} failed.")
                if success and not progress.cancelled and op_name in ["Update", "Upgrade"] and self.auto_restart.get_active():
                    GLib.idle_add(self.on_restart, None)
            except Exception as e:
                progress.append_output(f"Fatal error: {e}")
                progress.finish(False, f"Error during {op_name.lower()}: {e}")
        threading.Thread(target=run, daemon=True).start()
    def on_update(self, btn):
        # Integrate with update.py backend
        try:
            import importlib.util
            update_path = os.path.join(os.path.dirname(__file__), 'update.py')
            spec = importlib.util.spec_from_file_location('update', update_path)
            update_mod = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(update_mod)
            self.run_in_thread("Update", lambda: update_mod.run_update(self.distro))
        except Exception as e:
            self.show_error(f"Failed to run update: {e}")
    def on_upgrade(self, btn):
        # TODO: Integrate with upgrade.py backend
        self.run_in_thread("Upgrade", lambda: print("Upgrade logic here"))
    def on_setup(self, btn):
        # TODO: Integrate with setup.py backend
        self.run_in_thread("Setup", lambda: print("Setup logic here"))
    def on_restart(self, btn):
        dialog = Gtk.MessageDialog(transient_for=self, modal=True, message_type=Gtk.MessageType.QUESTION, buttons=Gtk.ButtonsType.YES_NO, text="Restart System")
        dialog.set_secondary_text("Are you sure you want to restart the system?")
        dialog.connect("response", self._on_restart_response)
        dialog.present()
    def _on_restart_response(self, dialog, response):
        dialog.destroy()
        if response == Gtk.ResponseType.YES:
            try:
                if os.name == 'nt':
                    os.system('shutdown /r /t 0')
                else:
                    import subprocess
                    subprocess.run(['sudo', 'reboot'], check=True)
            except Exception as e:
                self.show_error(f"Failed to restart: {e}")
    def show_error(self, msg):
        dialog = Gtk.MessageDialog(transient_for=self, modal=True, message_type=Gtk.MessageType.ERROR, buttons=Gtk.ButtonsType.OK, text="Error")
        dialog.set_secondary_text(msg)
        dialog.connect("response", lambda d, r: d.destroy())
        dialog.present()

# --- GTK Application ---
class UpApp(Gtk.Application):
    def __init__(self):
        super().__init__(application_id="org.up.app")
        self.add_main_option("cli", ord("c"), GLib.OptionFlags.NONE, GLib.OptionArg.NONE, "Run in CLI mode", None)
    def do_startup(self):
        Gtk.Application.do_startup(self)
        quit_action = Gio.SimpleAction.new("quit", None)
        quit_action.connect("activate", lambda a, p: self.quit())
        self.add_action(quit_action)
        about_action = Gio.SimpleAction.new("about", None)
        about_action.connect("activate", self.on_about)
        self.add_action(about_action)
        logs_action = Gio.SimpleAction.new("logs", None)
        logs_action.connect("activate", self.on_logs)
        self.add_action(logs_action)
        self.set_accels_for_action("app.quit", ["<Ctrl>Q"])
    def do_activate(self):
        win = UpWindow(self)
        win.present()
    def do_command_line(self, cmdline):
        opts = cmdline.get_options_dict()
        if opts.contains("cli"):
            from up import main_menu
            main_menu()
            return 0
        self.activate()
        return 0
    def on_about(self, action, param):
        about = Gtk.AboutDialog()
        about.set_program_name("Up")
        about.set_version("1.0")
        about.set_comments("Universal cross-distro Linux maintenance application")
        about.set_website("https://github.com/yourusername/up")
        about.set_copyright("Copyright © 2025")
        about.set_license_type(Gtk.License.MIT_X11)
        win = self.get_active_window()
        if win:
            about.set_transient_for(win)
            about.set_modal(True)
        about.present()
    def on_logs(self, action, param):
        win = self.get_active_window()
        if win:
            logwin = LogWindow(win)
            logwin.present()

def main():
    app = UpApp()
    return app.run(sys.argv)

if __name__ == "__main__":
    main()
