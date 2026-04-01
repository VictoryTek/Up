use adw::prelude::*;

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
        // Register the resource path so GTK's icon theme looks in the compiled GLib
        // resource bundle before any file-system location. This ensures the icon
        // embedded at build time is always used, regardless of what is installed
        // on the host system.
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::IconTheme::for_display(&display).add_resource_path("/io/github/up");
        }

        gtk::Window::set_default_icon_name("io.github.up");

        let window = UpWindow::build(app);
        window.present();
    }
}
