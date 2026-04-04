use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use crate::backends::Backend;

#[derive(Clone)]
pub struct UpdateRow {
    pub row: adw::ExpanderRow,
    status_label: gtk::Label,
    spinner: gtk::Spinner,
    progress_bar: gtk::ProgressBar,
    /// Tracks child rows added by set_packages() so they can be cleared on re-check.
    pkg_rows: Rc<RefCell<Vec<adw::ActionRow>>>,
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

        let row = adw::ExpanderRow::builder()
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
            pkg_rows: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// Populate the expander with a list of pending package names.
    /// Clears any previously added rows before adding new ones.
    /// Caps display at 50 items with a summary row for the remainder.
    pub fn set_packages(&self, packages: &[String]) {
        // Remove previously added package rows to avoid duplicates on re-check.
        {
            let mut tracked = self.pkg_rows.borrow_mut();
            for pkg_row in tracked.drain(..) {
                self.row.remove(&pkg_row);
            }
        }
        if packages.is_empty() {
            return;
        }
        const MAX_PACKAGES: usize = 50;
        let display_count = packages.len().min(MAX_PACKAGES);
        let mut tracked = self.pkg_rows.borrow_mut();
        for pkg in &packages[..display_count] {
            let pkg_row = adw::ActionRow::builder().title(pkg.as_str()).build();
            self.row.add_row(&pkg_row);
            tracked.push(pkg_row);
        }
        if packages.len() > MAX_PACKAGES {
            let remaining = packages.len() - MAX_PACKAGES;
            let more_row = adw::ActionRow::builder()
                .title(format!("\u{2026} and {remaining} more").as_str())
                .build();
            self.row.add_row(&more_row);
            tracked.push(more_row);
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
