use adw::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::backends::{Backend, BackendKind};

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
    backend_kind: BackendKind,
    packages_cache: Rc<RefCell<Vec<String>>>,
    changelog_row: adw::ActionRow,
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

        let backend_kind = backend.kind();
        let packages_cache: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

        let changelog_row = adw::ActionRow::builder()
            .title("View Changelog")
            .visible(false)
            .build();

        let changelog_button = gtk::Button::from_icon_name("document-open-symbolic");
        changelog_button.set_valign(gtk::Align::Center);
        changelog_button.add_css_class("flat");
        changelog_button.set_focusable(false);
        changelog_button.update_property(&[gtk::accessible::Property::Label("View Changelog")]);
        changelog_row.add_suffix(&changelog_button);

        if backend_kind != BackendKind::Nix {
            let packages_cache_click = packages_cache.clone();
            let row_weak = row.downgrade();
            let btn_weak = changelog_button.downgrade();

            changelog_button.connect_clicked(move |btn| {
                let pkgs: Vec<String> = packages_cache_click.borrow().clone();
                let kind = backend_kind;

                btn.set_sensitive(false);

                let (tx, rx) = async_channel::bounded(1);
                crate::runtime::runtime().spawn(async move {
                    let result = crate::changelog::fetch_changelog(kind, &pkgs).await;
                    let _ = tx.send(result).await;
                });

                let row_ref = row_weak.clone();
                let btn_ref = btn_weak.clone();
                glib::spawn_future_local(async move {
                    if let Ok(result) = rx.recv().await {
                        if let Some(btn) = btn_ref.upgrade() {
                            btn.set_sensitive(true);
                        }
                        let text = match result {
                            Ok(t) => t,
                            Err(crate::changelog::ChangelogError::NotSupported) => return,
                            Err(e) => format!("Error fetching changelog:\n{e}"),
                        };

                        let heading = match kind {
                            BackendKind::Pacman | BackendKind::Zypper => "Package Info",
                            _ => "Changelog",
                        };

                        let dialog = adw::AlertDialog::builder()
                            .heading(heading)
                            .body("")
                            .build();

                        let text_view = gtk::TextView::builder()
                            .editable(false)
                            .cursor_visible(false)
                            .wrap_mode(gtk::WrapMode::Word)
                            .monospace(true)
                            .margin_top(8)
                            .margin_bottom(8)
                            .margin_start(8)
                            .margin_end(8)
                            .build();
                        text_view.buffer().set_text(&text);

                        let scrolled = gtk::ScrolledWindow::builder()
                            .child(&text_view)
                            .min_content_height(300)
                            .max_content_height(500)
                            .hscrollbar_policy(gtk::PolicyType::Never)
                            .build();
                        dialog.set_extra_child(Some(&scrolled));
                        dialog.add_response("close", "Close");
                        dialog.set_default_response(Some("close"));
                        dialog.set_close_response("close");

                        let parent = row_ref.upgrade();
                        dialog.present(parent.as_ref());
                    }
                });
            });
        }

        row.add_row(&changelog_row);

        Self {
            row,
            status_label,
            spinner,
            pkg_rows: Rc::new(RefCell::new(Vec::new())),
            skip_flag,
            last_available,
            skip_checkbox,
            retry_button,
            backend_kind,
            packages_cache,
            changelog_row,
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
        // Update the cache used by the changelog button.
        *self.packages_cache.borrow_mut() = packages.to_vec();

        // Remove previously added package rows to avoid duplicates on re-check.
        {
            let mut tracked = self.pkg_rows.borrow_mut();
            for pkg_row in tracked.drain(..) {
                self.row.remove(&pkg_row);
            }
        }

        // Remove the changelog row so it can be re-appended at the bottom.
        self.row.remove(&self.changelog_row);

        // Hide the expand arrow when there is nothing to expand.
        self.row.set_enable_expansion(!packages.is_empty());
        if packages.is_empty() {
            self.row.set_expanded(false);
            self.changelog_row.set_visible(false);
            self.row.add_row(&self.changelog_row);
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
        // Re-append changelog row at the bottom; hidden for Nix (which returns
        // an empty package list anyway, but guard defensively).
        self.changelog_row
            .set_visible(self.backend_kind != BackendKind::Nix);
        self.row.add_row(&self.changelog_row);
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

    pub fn set_status_cancelled(&self) {
        self.retry_button.set_visible(false);
        self.skip_checkbox.set_sensitive(true);
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.status_label.set_label("Cancelled");
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
