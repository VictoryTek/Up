use adw::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use crate::backends::Backend;

#[derive(Clone)]
pub struct UpdateRow {
    pub row: adw::ExpanderRow,
    status_label: gtk::Label,
    spinner: gtk::Spinner,
    progress_bar: gtk::ProgressBar,
    /// Tracks child rows added by set_packages() so they can be cleared on re-check.
    pkg_rows: Rc<RefCell<Vec<adw::ActionRow>>>,
    progress_timer: Rc<RefCell<Option<glib::SourceId>>>,
    progress_fraction: Rc<Cell<f64>>,
}

impl UpdateRow {
    pub fn new(backend: &dyn Backend) -> Self {
        let status_label = gtk::Label::builder()
            .label("Ready")
            .css_classes(vec!["dim-label"])
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(30)
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
            progress_timer: Rc::new(RefCell::new(None)),
            progress_fraction: Rc::new(Cell::new(0.0)),
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
        // Hide the expand arrow when there is nothing to expand.
        self.row.set_enable_expansion(!packages.is_empty());
        if packages.is_empty() {
            self.row.set_expanded(false);
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

    pub fn set_status_running(&self) {
        // Cancel any previously running timer before starting a new one.
        if let Some(source_id) = self.progress_timer.borrow_mut().take() {
            source_id.remove();
        }
        self.progress_fraction.set(0.0);
        self.spinner.set_visible(true);
        self.spinner.set_spinning(true);
        self.progress_bar.set_visible(true);
        self.progress_bar.set_fraction(0.0);
        self.status_label.set_label("Updating...");
        self.status_label.set_css_classes(&["accent"]);

        let progress_bar = self.progress_bar.clone();
        let fraction_rc = self.progress_fraction.clone();

        let source_id = glib::timeout_add_local(Duration::from_millis(200), move || {
            let current = fraction_rc.get();
            let new_val = (current + 0.005).min(0.95);
            fraction_rc.set(new_val);
            progress_bar.set_fraction(new_val);
            glib::ControlFlow::Continue
        });

        *self.progress_timer.borrow_mut() = Some(source_id);
    }

    fn stop_progress_timer(&self) {
        if let Some(source_id) = self.progress_timer.borrow_mut().take() {
            source_id.remove();
        }
        self.progress_fraction.set(1.0);
        self.progress_bar.set_fraction(1.0);
    }

    pub fn set_status_success(&self, count: usize) {
        self.stop_progress_timer();
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
        self.stop_progress_timer();
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.progress_bar.set_visible(false);
        self.status_label.set_label(&format!("Error: {}", msg));
        self.status_label.set_css_classes(&["error"]);
    }

    pub fn set_status_skipped(&self, msg: &str) {
        self.stop_progress_timer();
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.progress_bar.set_visible(false);
        self.status_label.set_label(msg);
        self.status_label.set_css_classes(&["dim-label"]);
    }

    /// Used when the count cannot be determined without running the update (e.g. NixOS).
    pub fn set_status_unknown(&self, msg: &str) {
        self.stop_progress_timer();
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.progress_bar.set_visible(false);
        self.status_label.set_label(msg);
        self.status_label.set_css_classes(&["dim-label"]);
    }
}
