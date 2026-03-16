use adw::prelude::*;
use gtk::prelude::*;

use crate::backends::Backend;

#[derive(Clone)]
pub struct UpdateRow {
    pub row: adw::ActionRow,
    status_label: gtk::Label,
    spinner: gtk::Spinner,
    progress_bar: gtk::ProgressBar,
}

impl UpdateRow {
    pub fn new(backend: &dyn Backend) -> Self {
        let status_label = gtk::Label::builder()
            .label("Ready")
            .css_classes(vec!["dim-label"])
            .build();

        let spinner = gtk::Spinner::builder().visible(false).build();

        let progress_bar = gtk::ProgressBar::builder()
            .visible(false)
            .valign(gtk::Align::Center)
            .width_request(100)
            .build();

        let icon = gtk::Image::from_icon_name(backend.icon_name());

        let row = adw::ActionRow::builder()
            .title(backend.display_name())
            .subtitle(backend.description())
            .build();

        row.add_prefix(&icon);
        row.add_suffix(&spinner);
        row.add_suffix(&progress_bar);
        row.add_suffix(&status_label);

        Self {
            row,
            status_label,
            spinner,
            progress_bar,
        }
    }

    pub fn set_status_checking(&self) {
        self.spinner.set_visible(true);
        self.spinner.set_spinning(true);
        self.progress_bar.set_visible(false);
        self.status_label.set_label("Checking...");
        self.status_label.set_css_classes(&["dim-label"]);
    }

    pub fn set_status_available(&self, count: usize) {
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        if count == 0 {
            self.status_label.set_label("Up to date");
            self.status_label.set_css_classes(&["success"]);
        } else {
            self.status_label.set_label(&format!("{count} available"));
            self.status_label.set_css_classes(&["accent"]);
        }
    }

    pub fn pulse_progress(&self) {
        self.progress_bar.pulse();
    }

    pub fn set_status_running(&self) {
        self.spinner.set_visible(true);
        self.spinner.set_spinning(true);
        self.progress_bar.set_visible(true);
        self.progress_bar.set_fraction(0.0);
        self.status_label.set_label("Updating...");
        self.status_label.set_css_classes(&["accent"]);
    }

    pub fn set_status_success(&self, count: usize) {
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.progress_bar.set_visible(false);
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
        self.progress_bar.set_visible(false);
        self.status_label.set_label(&format!("Error: {}", msg));
        self.status_label.set_css_classes(&["error"]);
    }

    pub fn set_status_skipped(&self, msg: &str) {
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.progress_bar.set_visible(false);
        self.status_label.set_label(msg);
        self.status_label.set_css_classes(&["dim-label"]);
    }

    /// Used when the count cannot be determined without running the update (e.g. NixOS).
    pub fn set_status_unknown(&self, msg: &str) {
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.progress_bar.set_visible(false);
        self.status_label.set_label(msg);
        self.status_label.set_css_classes(&["dim-label"]);
    }
}
