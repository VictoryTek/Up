use adw::prelude::*;
use gtk::prelude::*;

use crate::backends::{Backend, BackendKind};

#[derive(Clone)]
pub struct UpdateRow {
    pub row: adw::ActionRow,
    status_label: gtk::Label,
    spinner: gtk::Spinner,
}

impl UpdateRow {
    pub fn new(backend: &dyn Backend) -> Self {
        let status_label = gtk::Label::builder()
            .label("Ready")
            .css_classes(vec!["dim-label"])
            .build();

        let spinner = gtk::Spinner::builder()
            .visible(false)
            .build();

        let icon = gtk::Image::from_icon_name(backend.icon_name());

        let row = adw::ActionRow::builder()
            .title(backend.display_name())
            .subtitle(backend.description())
            .build();

        row.add_prefix(&icon);
        row.add_suffix(&spinner);
        row.add_suffix(&status_label);

        Self {
            row,
            status_label,
            spinner,
        }
    }

    pub fn set_status_running(&self) {
        self.spinner.set_visible(true);
        self.spinner.set_spinning(true);
        self.status_label.set_label("Updating...");
        self.status_label.set_css_classes(&["accent"]);
    }

    pub fn set_status_success(&self, count: usize) {
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        let msg = if count == 0 {
            "Up to date".to_string()
        } else {
            format!("{count} updated")
        };
        self.status_label.set_label(&msg);
        self.status_label.set_css_classes(&["success"]);
    }

    pub fn set_status_error(&self, msg: &str) {
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.status_label.set_label(&format!("Error: {}", msg));
        self.status_label.set_css_classes(&["error"]);
    }

    pub fn set_status_skipped(&self, msg: &str) {
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.status_label.set_label(msg);
        self.status_label.set_css_classes(&["dim-label"]);
    }
}
