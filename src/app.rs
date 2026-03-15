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
        // Add icon search path for development (cargo run from project root)
        if let Some(display) = gtk::gdk::Display::default() {
            let theme = gtk::IconTheme::for_display(&display);
            theme.add_search_path("data/icons");
        }

        gtk::Window::set_default_icon_name("io.github.up");

        let window = UpWindow::new(app);
        window.present();
    }
}
