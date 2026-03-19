mod app;
mod backends;
mod reboot;
mod runner;
mod ui;
mod upgrade;

use app::UpApplication;

const APP_ID: &str = "io.github.up";

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let app = UpApplication::new();
    app.run();
}
