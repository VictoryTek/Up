mod app;
mod ui;
mod backends;
mod upgrade;
mod runner;

use app::UpApplication;

const APP_ID: &str = "io.github.up";

fn main() {
    env_logger::init();
    let app = UpApplication::new();
    app.run();
}
