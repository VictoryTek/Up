use adw::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

use crate::backends::Backend;

#[derive(Clone)]
pub struct UpdateRow {
    pub row: adw::ActionRow,
    status_label: gtk::Label,
    spinner: gtk::Spinner,
    /// Opens the popover listing this backend's pending/updated packages.
    menu_button: gtk::MenuButton,
    /// Heading inside the popover, e.g. "NixOS — 42 packages".
    popover_heading: gtk::Label,
    /// Holds one row per package name shown in the popover; cleared and
    /// repopulated on each set_packages() call.
    popover_list: gtk::ListBox,
    /// Backend display name, reused for the popover heading.
    backend_name: String,
    /// Current skip state; toggled by the skip checkbox.
    skip_flag: Rc<Cell<bool>>,
    /// Last resolved available-update count; used to restore status on un-skip.
    last_available: Rc<Cell<Option<usize>>>,
    /// Set when the most recent check returned an error; reset when a new check starts.
    /// Lets the window distinguish "0 updates confirmed" from "check failed".
    check_errored: Rc<Cell<bool>>,
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

        let backend_name = backend.display_name().to_string();

        let row = adw::ActionRow::builder()
            .title(backend.display_name())
            .subtitle(backend.description())
            .build();

        let retry_button = gtk::Button::from_icon_name("view-refresh-symbolic");
        retry_button.set_tooltip_text(Some("Retry"));
        retry_button.set_visible(false);
        retry_button.connect_clicked(move |_| on_retry());

        let popover_heading = gtk::Label::builder()
            .css_classes(vec!["heading"])
            .halign(gtk::Align::Start)
            .margin_bottom(6)
            .build();

        let popover_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(vec!["boxed-list", "pkg-popover-list"])
            .build();

        let popover_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .max_content_height(320)
            .propagate_natural_height(true)
            .child(&popover_list)
            .build();

        let popover_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_top(10)
            .margin_bottom(10)
            .margin_start(10)
            .margin_end(10)
            .width_request(240)
            .build();
        popover_box.append(&popover_heading);
        popover_box.append(&popover_scroller);

        let popover = gtk::Popover::builder().child(&popover_box).build();

        let menu_button = gtk::MenuButton::builder()
            .valign(gtk::Align::Center)
            .css_classes(vec!["pkg-count-pill"])
            .visible(false)
            .build();
        menu_button.set_popover(Some(&popover));

        row.add_prefix(&icon);
        row.add_suffix(&menu_button);
        row.add_suffix(&status_label);
        row.add_suffix(&spinner);
        row.add_suffix(&retry_button);
        row.add_suffix(&skip_checkbox);

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
            menu_button,
            popover_heading,
            popover_list,
            backend_name,
            skip_flag,
            last_available,
            check_errored: Rc::new(Cell::new(false)),
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

    /// Returns `true` if the most recent check produced an error.
    /// Reset to `false` at the start of each check cycle via `set_status_checking()`.
    pub fn has_check_error(&self) -> bool {
        self.check_errored.get()
    }

    /// Populate the popover with a list of pending package names.
    /// Clears any previously added rows before adding new ones.
    /// Caps display at 50 items with a summary row for the remainder.
    pub fn set_packages(&self, packages: &[String]) {
        // Remove previously added package rows to avoid duplicates on re-check.
        while let Some(child) = self.popover_list.first_child() {
            self.popover_list.remove(&child);
        }
        // Hide the pill button when there is nothing to show.
        if packages.is_empty() {
            self.menu_button.set_visible(false);
            return;
        }
        self.menu_button
            .set_label(&format!("{} pkgs", packages.len()));
        self.menu_button.set_visible(true);
        self.popover_heading.set_label(&format!(
            "{} \u{2014} {} packages",
            self.backend_name,
            packages.len()
        ));

        const MAX_PACKAGES: usize = 50;
        let display_count = packages.len().min(MAX_PACKAGES);
        for pkg in &packages[..display_count] {
            let label = gtk::Label::builder()
                .label(pkg.as_str())
                .halign(gtk::Align::Start)
                .build();
            self.popover_list.append(&label);
        }
        if packages.len() > MAX_PACKAGES {
            let remaining = packages.len() - MAX_PACKAGES;
            let label = gtk::Label::builder()
                .label(format!("\u{2026} and {remaining} more"))
                .halign(gtk::Align::Start)
                .css_classes(vec!["dim-label"])
                .build();
            self.popover_list.append(&label);
        }
    }

    pub fn set_status_checking(&self) {
        self.retry_button.set_visible(false);
        self.last_available.set(None);
        self.check_errored.set(false);
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

    /// Used when the count cannot be determined (e.g. NixOS) or a check error occurred.
    /// Sets `check_errored` so the window can avoid a false "Everything is up to date."
    pub fn set_status_unknown(&self, msg: &str) {
        self.retry_button.set_visible(false);
        self.skip_checkbox.set_sensitive(true);
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.check_errored.set(true);
        self.status_label.set_label(msg);
        self.status_label.set_css_classes(&["dim-label"]);
    }
}
