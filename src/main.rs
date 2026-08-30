mod app;
mod backends;
mod battery;
mod changelog;
mod check;
mod config;
mod disk;
mod executor;
mod history;
mod orchestrator;
mod plugins;
mod progress;
mod reboot;
mod runner;
mod runtime;
mod ui;
mod upgrade;

use app::UpApplication;

const APP_ID: &str = "io.github.up";

/// Bind the gettext text domain so `gettext!`/`gettext()` calls resolve against
/// the installed `.mo` catalogs. `LOCALEDIR` is provided by the meson build;
/// a plain `cargo` build falls back to the FHS default.
fn init_gettext() {
    use gettextrs::{
        bind_textdomain_codeset, bindtextdomain, setlocale, textdomain, LocaleCategory,
    };

    let localedir = option_env!("LOCALEDIR").unwrap_or("/usr/share/locale");
    setlocale(LocaleCategory::LcAll, "");
    let _ = bindtextdomain(APP_ID, localedir);
    let _ = bind_textdomain_codeset(APP_ID, "UTF-8");
    let _ = textdomain(APP_ID);
}

fn main() -> gtk::glib::ExitCode {
    env_logger::init();
    init_gettext();

    if std::env::args().any(|a| a == "--check") {
        check::run_check();
        return gtk::glib::ExitCode::SUCCESS;
    }

    gio::resources_register_include!("compiled.gresource").expect("Failed to register resources.");
    let app = UpApplication::new();
    app.run()
}
