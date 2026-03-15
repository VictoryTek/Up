use adw::prelude::*;
use gtk::gio;
use gtk::prelude::*;

use crate::ui::window::UpWindow;
use crate::APP_ID;

pub struct UpApplication {
    app: adw::Application,
}

impl UpApplication {
    pub fn new() -> Self {
        let app = adw::Application::builder().application_id(APP_ID).build();

        app.connect_activate(Self::on_activate);

        Self { app }
    }

    pub fn run(&self) -> gtk::glib::ExitCode {
        self.app.run()
    }

    fn on_activate(app: &adw::Application) {
        // Add local icon search path when running from the project root (development mode).
        // CARGO_MANIFEST_DIR is a compile-time absolute path; we only add it if the directory
        // still exists at runtime, so installed/Flatpak builds are unaffected.
        let dev_icons = concat!(env!("CARGO_MANIFEST_DIR"), "/data/icons");
        if std::path::Path::new(dev_icons).exists() {
            if let Some(display) = gtk::gdk::Display::default() {
                gtk::IconTheme::for_display(&display).add_search_path(dev_icons);
            }
        }

        gtk::Window::set_default_icon_name("io.github.up");

        let window = UpWindow::new(app);
        window.present();
    }
}
