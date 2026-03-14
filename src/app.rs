use adw::prelude::*;
use gtk::prelude::*;
use gtk::gio;

use crate::ui::window::UpWindow;
use crate::APP_ID;

pub struct UpApplication {
    app: adw::Application,
}

impl UpApplication {
    pub fn new() -> Self {
        let app = adw::Application::builder()
            .application_id(APP_ID)
            .build();

        app.connect_activate(Self::on_activate);

        Self { app }
    }

    pub fn run(&self) -> gtk::glib::ExitCode {
        self.app.run()
    }

    fn on_activate(app: &adw::Application) {
        let window = UpWindow::new(app);
        window.present();
    }
}
