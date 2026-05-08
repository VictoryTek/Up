use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::backends::Backend;

#[derive(Clone)]
pub struct UpdateRow {
    pub row: adw::ExpanderRow,
    status_label: gtk::Label,
    spinner: gtk::Spinner,
    /// Tracks child rows added by set_packages() so they can be cleared on re-check.
    pkg_rows: Rc<RefCell<Vec<adw::ActionRow>>>,
    /// Current skip state; toggled by the skip checkbox.
    skip_flag: Rc<Cell<bool>>,
    /// Last resolved available-update count; used to restore status on un-skip.
    last_available: Rc<Cell<Option<usize>>>,
    skip_checkbox: gtk::CheckButton,
    retry_button: gtk::Button,
}

impl UpdateRow {
    pub fn new(
        backend: &dyn Backend,
        on_skip_changed: impl Fn() + 'static,
        on_retry: impl Fn() + 'static,
    ) -> Self {
        let status_label = gtk::Label::builder()
            .label("Ready")
            .css_classes(vec!["dim-label"])
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(30)
            .build();

        let spinner = gtk::Spinner::builder().visible(false).build();

        let icon = gtk::Image::builder()
            .icon_name(backend.icon_name())
            .accessible_role(gtk::AccessibleRole::Presentation)
            .build();

        let skip_flag = Rc::new(Cell::new(false));
        let last_available: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));

        let kind_label = format!("Skip {} during Update All", backend.display_name());
        let skip_checkbox = gtk::CheckButton::builder()
            .tooltip_text(&kind_label)
            .valign(gtk::Align::Center)
            .build();
        skip_checkbox.update_property(&[gtk::accessible::Property::Label(kind_label.as_str())]);

        let row = adw::ExpanderRow::builder()
            .title(backend.display_name())
            .subtitle(backend.description())
            .build();

        let retry_button = gtk::Button::from_icon_name("view-refresh-symbolic");
        retry_button.set_tooltip_text(Some("Retry"));
        retry_button.set_visible(false);
        retry_button.connect_clicked(move |_| on_retry());

        row.add_prefix(&icon);
        row.add_suffix(&skip_checkbox);
        row.add_suffix(&retry_button);
        row.add_suffix(&spinner);
        row.add_suffix(&status_label);

        {
            let skip_flag = skip_flag.clone();
            let last_available = last_available.clone();
            let status_label = status_label.clone();
            skip_checkbox.connect_toggled(move |cb| {
                let skipped = cb.is_active();
                skip_flag.set(skipped);
                if skipped {
                    status_label.set_label("Skipped");
                    status_label.set_css_classes(&["dim-label"]);
                } else {
                    match last_available.get() {
                        Some(count) => {
                            if count == 0 {
                                status_label.set_label("Up to date");
                                status_label.set_css_classes(&["success"]);
                            } else {
                                status_label.set_label(&format!("{count} available"));
                                status_label.set_css_classes(&["accent"]);
                            }
                        }
                        None => {
                            status_label.set_label("Ready");
                            status_label.set_css_classes(&["dim-label"]);
                        }
                    }
                }
                on_skip_changed();
            });
        }

        Self {
            row,
            status_label,
            spinner,
            pkg_rows: Rc::new(RefCell::new(Vec::new())),
            skip_flag,
            last_available,
            skip_checkbox,
            retry_button,
        }
    }

    /// Returns `true` if the user has checked this backend's skip box.
    pub fn is_skipped(&self) -> bool {
        self.skip_flag.get()
    }

    /// Returns the last resolved available-update count for this backend.
    /// `None` if no successful check has completed yet.
    pub fn last_available_count(&self) -> Option<usize> {
        self.last_available.get()
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
        self.retry_button.set_visible(false);
        self.last_available.set(None);
        self.spinner.set_visible(true);
        self.spinner.set_spinning(true);
        self.status_label.set_label("Checking...");
        self.status_label.set_css_classes(&["dim-label"]);
    }

    pub fn set_status_available(&self, count: usize) {
        self.retry_button.set_visible(false);
        self.last_available.set(Some(count));
        self.skip_checkbox.set_sensitive(true);
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
        self.retry_button.set_visible(false);
        self.skip_checkbox.set_sensitive(false);
        self.spinner.set_visible(true);
        self.spinner.set_spinning(true);
        self.status_label.set_label("Updating...");
        self.status_label.set_css_classes(&["accent"]);
    }

    pub fn set_status_success(&self, count: usize) {
        self.retry_button.set_visible(false);
        self.skip_checkbox.set_sensitive(true);
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
        self.retry_button.set_visible(true);
        self.skip_checkbox.set_sensitive(true);
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.status_label.set_label(&format!("Error: {}", msg));
        self.status_label.set_css_classes(&["error"]);
    }

    pub fn set_status_skipped(&self, msg: &str) {
        self.retry_button.set_visible(false);
        self.skip_checkbox.set_sensitive(true);
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.status_label.set_label(msg);
        self.status_label.set_css_classes(&["dim-label"]);
    }

    /// Used when the count cannot be determined without running the update (e.g. NixOS).
    pub fn set_status_unknown(&self, msg: &str) {
        self.retry_button.set_visible(false);
        self.skip_checkbox.set_sensitive(true);
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.status_label.set_label(msg);
        self.status_label.set_css_classes(&["dim-label"]);
    }

    pub fn set_status_cleaning(&self) {
        self.retry_button.set_visible(false);
        self.skip_checkbox.set_sensitive(false);
        self.spinner.set_visible(true);
        self.spinner.set_spinning(true);
        self.status_label.set_label("Cleaning\u{2026}");
        self.status_label.set_css_classes(&["accent"]);
    }

    pub fn set_status_cleaned(&self, removed: usize) {
        self.retry_button.set_visible(false);
        self.skip_checkbox.set_sensitive(true);
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        let msg = if removed == 0 {
            "Already clean".to_string()
        } else {
            format!("{removed} removed")
        };
        self.status_label.set_label(&msg);
        self.status_label.set_css_classes(&["success"]);
    }

    /// Restore persisted skip state on startup.
    /// Triggers the same visual update and on_skip_changed callback as a user click.
    pub fn set_skipped(&self, skipped: bool) {
        self.skip_checkbox.set_active(skipped);
    }
}
