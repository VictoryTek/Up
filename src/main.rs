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
mod reboot;
mod runner;
mod runtime;
mod snapshot;
mod ui;
mod upgrade;

use app::UpApplication;

const APP_ID: &str = "io.github.up";

fn main() -> gtk::glib::ExitCode {
    env_logger::init();

    if std::env::args().any(|a| a == "--check") {
        check::run_check();
        return gtk::glib::ExitCode::SUCCESS;
    }

    gio::resources_register_include!("compiled.gresource").expect("Failed to register resources.");
    let app = UpApplication::new();
    app.run()
}
