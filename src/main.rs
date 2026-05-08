mod app;
mod backends;
mod battery;
mod changelog;
mod config;
mod executor;
mod history;
mod orchestrator;
mod reboot;
mod runner;
mod runtime;
mod snapshot;
mod ui;
mod upgrade;

use app::UpApplication;
use gettextrs::{bindtextdomain, setlocale, textdomain, LocaleCategory};

const APP_ID: &str = "io.github.up";

fn main() -> gtk::glib::ExitCode {
    // i18n — must be before GTK/adw initialization
    setlocale(LocaleCategory::LcAll, "");
    let localedir = option_env!("LOCALEDIR").unwrap_or("/usr/share/locale");
    bindtextdomain(APP_ID, localedir).expect("Unable to bind the text domain");
    textdomain(APP_ID).expect("Unable to switch to the text domain");

    gio::resources_register_include!("compiled.gresource").expect("Failed to register resources.");
    env_logger::init();
    let app = UpApplication::new();
    app.run()
}
