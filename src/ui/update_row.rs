use adw::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
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
    /// Base subtitle text (backend.description()); preserved so that size
    /// information can be appended and later removed on re-check.
    base_description: String,
    /// Last estimated required disk space returned by `estimate_size()`.
    /// `None` means the backend does not support estimation or has not yet run.
    estimated_bytes: Rc<Cell<Option<u64>>>,
    /// Whether the backend supports per-item selection (set once at construction).
    supports_item_selection: bool,
    /// Tracks item IDs that the user has DESELECTED (excluded from update).
    deselected_items: Rc<RefCell<HashSet<String>>>,
    /// All item IDs loaded by the most recent set_packages() call.
    all_item_ids: Rc<RefCell<Vec<String>>>,
    /// CheckButton widgets for displayed items (up to MAX_PACKAGES).
    child_checkboxes: Rc<RefCell<Vec<gtk::CheckButton>>>,
    /// Guard flag: true while parent checkbox state is being updated programmatically.
    updating_parent: Rc<Cell<bool>>,
    /// Callback invoked when per-item selection changes.
    on_selection_changed: Rc<dyn Fn()>,
    /// True when the last check returned an error (set_status_unknown was called).
    check_errored: Rc<Cell<bool>>,
}

impl UpdateRow {
    pub fn new(
        backend: &dyn Backend,
        on_skip_changed: impl Fn() + 'static,
        on_retry: impl Fn() + 'static,
        on_selection_changed: impl Fn() + 'static,
    ) -> Self {
        let status_label = gtk::Label::builder()
            .label(gettext("Ready"))
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

        let kind_label = gettext("Skip {} during Update All").replace("{}", backend.display_name());
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
        retry_button.set_tooltip_text(Some(&gettext("Retry")));
        retry_button.set_visible(false);
        retry_button.connect_clicked(move |_| on_retry());

        row.add_prefix(&icon);
        row.add_suffix(&skip_checkbox);
        row.add_suffix(&retry_button);
        row.add_suffix(&spinner);
        row.add_suffix(&status_label);

        let supports_item_selection = backend.supports_item_selection();
        let deselected_items: Rc<RefCell<HashSet<String>>> = Rc::new(RefCell::new(HashSet::new()));
        let all_item_ids: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let child_checkboxes: Rc<RefCell<Vec<gtk::CheckButton>>> =
            Rc::new(RefCell::new(Vec::new()));
        let updating_parent: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let on_selection_changed: Rc<dyn Fn()> = Rc::new(on_selection_changed);

        {
            let skip_flag = skip_flag.clone();
            let last_available = last_available.clone();
            let status_label = status_label.clone();
            let updating_parent = updating_parent.clone();
            let deselected_items = deselected_items.clone();
            let all_item_ids = all_item_ids.clone();
            let child_checkboxes = child_checkboxes.clone();
            let on_selection_changed_cb = on_selection_changed.clone();
            skip_checkbox.connect_toggled(move |cb| {
                // Ignore programmatic updates triggered by child-checkbox handlers.
                if updating_parent.get() {
                    return;
                }

                // When the parent is in an indeterminate state and the user clicks it,
                // interpret as "select all items" regardless of which direction active goes.
                if supports_item_selection && cb.is_inconsistent() {
                    deselected_items.borrow_mut().clear();
                    updating_parent.set(true);
                    cb.set_inconsistent(false);
                    cb.set_active(false);
                    for child_cb in child_checkboxes.borrow().iter() {
                        child_cb.set_active(true);
                    }
                    updating_parent.set(false);
                    skip_flag.set(false);
                    // Restore the appropriate status label.
                    match last_available.get() {
                        Some(count) if count > 0 => {
                            status_label.set_label(
                                &ngettext(
                                    "1 package available",
                                    "{} packages available",
                                    count as u32,
                                )
                                .replace("{}", &count.to_string()),
                            );
                            status_label.set_css_classes(&["accent"]);
                        }
                        Some(_) => {
                            status_label.set_label(&gettext("Up to date"));
                            status_label.set_css_classes(&["success"]);
                        }
                        None => {
                            status_label.set_label(&gettext("Ready"));
                            status_label.set_css_classes(&["dim-label"]);
                        }
                    }
                    on_skip_changed();
                    on_selection_changed_cb();
                    return;
                }

                let skipped = cb.is_active();
                skip_flag.set(skipped);

                if supports_item_selection {
                    if skipped {
                        // Parent fully skipped → deselect all children.
                        let ids = all_item_ids.borrow().clone();
                        *deselected_items.borrow_mut() = ids.into_iter().collect();
                        updating_parent.set(true);
                        for child_cb in child_checkboxes.borrow().iter() {
                            child_cb.set_active(false);
                        }
                        updating_parent.set(false);
                    } else {
                        // Parent un-skipped → re-select all children.
                        deselected_items.borrow_mut().clear();
                        updating_parent.set(true);
                        for child_cb in child_checkboxes.borrow().iter() {
                            child_cb.set_active(true);
                        }
                        updating_parent.set(false);
                    }
                    on_selection_changed_cb();
                }

                if skipped {
                    status_label.set_label(&gettext("Skipped"));
                    status_label.set_css_classes(&["dim-label"]);
                } else {
                    match last_available.get() {
                        Some(count) => {
                            if count == 0 {
                                status_label.set_label(&gettext("Up to date"));
                                status_label.set_css_classes(&["success"]);
                            } else {
                                status_label.set_label(
                                    &ngettext(
                                        "1 package available",
                                        "{} packages available",
                                        count as u32,
                                    )
                                    .replace("{}", &count.to_string()),
                                );
                                status_label.set_css_classes(&["accent"]);
                            }
                        }
                        None => {
                            status_label.set_label(&gettext("Ready"));
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
            .title(gettext("View Changelog"))
            .visible(false)
            .build();

        let changelog_button = gtk::Button::from_icon_name("document-open-symbolic");
        changelog_button.set_valign(gtk::Align::Center);
        changelog_button.add_css_class("flat");
        changelog_button.set_focusable(false);
        changelog_button
            .update_property(&[gtk::accessible::Property::Label(&gettext("View Changelog"))]);
        changelog_row.add_suffix(&changelog_button);

        if backend_kind != BackendKind::Nix {
            let packages_cache_click = packages_cache.clone();
            let row_weak = row.downgrade();
            let btn_weak = changelog_button.downgrade();
            let backend_kind_closure = backend_kind.clone();

            changelog_button.connect_clicked(move |btn| {
                let pkgs: Vec<String> = packages_cache_click.borrow().clone();
                let kind = backend_kind_closure.clone();
                let kind2 = kind.clone();

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

                        let heading = match kind2 {
                            BackendKind::Pacman | BackendKind::Zypper => gettext("Package Info"),
                            _ => gettext("Changelog"),
                        };

                        let dialog = adw::AlertDialog::builder()
                            .heading(&heading)
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
                        dialog.add_response("close", &gettext("Close"));
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
            base_description: backend.description().to_string(),
            estimated_bytes: Rc::new(Cell::new(None)),
            supports_item_selection,
            deselected_items,
            all_item_ids,
            child_checkboxes,
            updating_parent,
            on_selection_changed,
            check_errored: Rc::new(Cell::new(false)),
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

    /// Returns `true` if the last check ended in an error (set_status_unknown was called).
    pub fn check_errored(&self) -> bool {
        self.check_errored.get()
    }

    /// Returns the last estimated required disk bytes from `estimate_size()`.
    /// `None` means the backend does not support estimation or has not yet run.
    pub fn estimated_bytes(&self) -> Option<u64> {
        self.estimated_bytes.get()
    }

    /// Update the row subtitle to include the estimated required disk space.
    ///
    /// If `size_bytes` is `None` or 0, the subtitle reverts to the base description.
    /// Must be called on the GTK main thread.
    pub fn set_download_size(&self, size_bytes: Option<u64>) {
        self.estimated_bytes.set(size_bytes);
        let subtitle = match size_bytes {
            Some(bytes) if bytes > 0 => format!(
                "{} \u{2014} {} needed",
                self.base_description,
                crate::disk::format_bytes(bytes)
            ),
            _ => self.base_description.clone(),
        };
        self.row.set_subtitle(&subtitle);
    }

    /// Populate the expander with a list of pending package names.
    /// Clears any previously added rows before adding new ones.
    /// Caps display at 50 items with a summary row for the remainder.
    pub fn set_packages(&self, packages: &[String]) {
        // Update the cache used by the changelog button.
        *self.packages_cache.borrow_mut() = packages.to_vec();

        // Reset selection state: clear deselected set and repopulate all_item_ids.
        self.deselected_items.borrow_mut().clear();
        *self.all_item_ids.borrow_mut() = packages.to_vec();

        // Clear child checkbox refs from the previous call.
        self.child_checkboxes.borrow_mut().clear();

        // Remove previously added package rows to avoid duplicates on re-check.
        {
            let mut tracked = self.pkg_rows.borrow_mut();
            for pkg_row in tracked.drain(..) {
                self.row.remove(&pkg_row);
            }
        }

        // Remove the changelog row so it can be re-appended at the bottom.
        self.row.remove(&self.changelog_row);

        // Reset parent checkbox: all items are selected after a re-check.
        if self.supports_item_selection {
            self.updating_parent.set(true);
            self.skip_checkbox.set_inconsistent(false);
            self.updating_parent.set(false);
        }

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

            if self.supports_item_selection {
                let cb = gtk::CheckButton::builder()
                    .active(true)
                    .valign(gtk::Align::Center)
                    .build();
                let label = gettext("Include {} in update").replace("{}", pkg.as_str());
                cb.update_property(&[gtk::accessible::Property::Label(label.as_str())]);

                // Wire up per-item toggle: updates deselected_items and the parent checkbox.
                {
                    let pkg_id = pkg.clone();
                    let deselected = self.deselected_items.clone();
                    let all_ids = self.all_item_ids.clone();
                    let skip_cb = self.skip_checkbox.clone();
                    let skip_flag = self.skip_flag.clone();
                    let updating_parent = self.updating_parent.clone();
                    let on_sel = self.on_selection_changed.clone();
                    cb.connect_toggled(move |item_cb| {
                        // Parent is driving a bulk toggle — skip redundant updates.
                        if updating_parent.get() {
                            return;
                        }
                        if item_cb.is_active() {
                            deselected.borrow_mut().remove(&pkg_id);
                        } else {
                            deselected.borrow_mut().insert(pkg_id.clone());
                        }
                        // Derive and apply the new parent checkbox state.
                        updating_parent.set(true);
                        let total = all_ids.borrow().len();
                        let desel_count = deselected.borrow().len();
                        if desel_count == 0 {
                            // All selected → parent shows checkmark (not skipped).
                            skip_cb.set_inconsistent(false);
                            skip_cb.set_active(false);
                            skip_flag.set(false);
                        } else if desel_count < total {
                            // Partial → parent shows dash (indeterminate, not skipped).
                            skip_cb.set_inconsistent(true);
                            skip_cb.set_active(false);
                            skip_flag.set(false);
                        } else {
                            // All deselected → parent shows unchecked (backend skipped).
                            skip_cb.set_inconsistent(false);
                            skip_cb.set_active(true);
                            skip_flag.set(true);
                        }
                        updating_parent.set(false);
                        (*on_sel)();
                    });
                }

                pkg_row.add_prefix(&cb);
                pkg_row.set_activatable_widget(Some(&cb));
                self.child_checkboxes.borrow_mut().push(cb);
            }

            self.row.add_row(&pkg_row);
            tracked.push(pkg_row);
        }
        if packages.len() > MAX_PACKAGES {
            let remaining = packages.len() - MAX_PACKAGES;
            let more_row = adw::ActionRow::builder()
                .title(
                    ngettext(
                        "\u{2026} and 1 more",
                        "\u{2026} and {} more",
                        remaining as u32,
                    )
                    .replace("{}", &remaining.to_string())
                    .as_str(),
                )
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
        self.check_errored.set(false);
        self.last_available.set(None);
        self.estimated_bytes.set(None);
        self.row.set_subtitle(&self.base_description);
        self.spinner.set_visible(true);
        self.spinner.set_spinning(true);
        self.status_label.set_label(&gettext("Checking..."));
        self.status_label.set_css_classes(&["dim-label"]);
    }

    pub fn set_status_available(&self, count: usize) {
        self.retry_button.set_visible(false);
        self.last_available.set(Some(count));
        self.skip_checkbox.set_sensitive(true);
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        if count == 0 {
            self.status_label.set_label(&gettext("Up to date"));
            self.status_label.set_css_classes(&["success"]);
        } else {
            self.status_label.set_label(
                &ngettext("1 package available", "{} packages available", count as u32)
                    .replace("{}", &count.to_string()),
            );
            self.status_label.set_css_classes(&["accent"]);
        }
    }

    pub fn set_status_running(&self) {
        self.retry_button.set_visible(false);
        self.skip_checkbox.set_sensitive(false);
        self.spinner.set_visible(true);
        self.spinner.set_spinning(true);
        self.status_label.set_label(&gettext("Updating..."));
        self.status_label.set_css_classes(&["accent"]);
    }

    pub fn set_status_success(&self, count: usize) {
        self.retry_button.set_visible(false);
        self.skip_checkbox.set_sensitive(true);
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        let msg = if count == 0 {
            gettext("Up to date")
        } else {
            ngettext("1 package updated", "{} packages updated", count as u32)
                .replace("{}", &count.to_string())
        };
        self.status_label.set_label(&msg);
        self.status_label.set_css_classes(&["success"]);
    }

    pub fn set_status_error(&self, msg: &str) {
        self.retry_button.set_visible(true);
        self.skip_checkbox.set_sensitive(true);
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.status_label
            .set_label(&format!("{} {}", gettext("Error:"), msg));
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
        self.status_label.set_label(&gettext("Cancelled"));
        self.status_label.set_css_classes(&["dim-label"]);
    }

    /// Used when the count cannot be determined without running the update (e.g. NixOS).
    pub fn set_status_unknown(&self, msg: &str) {
        self.retry_button.set_visible(false);
        self.check_errored.set(true);
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
        self.status_label.set_label(&gettext("Cleaning\u{2026}"));
        self.status_label.set_css_classes(&["accent"]);
    }

    pub fn set_status_cleaned(&self, removed: usize) {
        self.retry_button.set_visible(false);
        self.skip_checkbox.set_sensitive(true);
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        let msg = if removed == 0 {
            gettext("Already clean")
        } else {
            ngettext("1 package removed", "{} packages removed", removed as u32)
                .replace("{}", &removed.to_string())
        };
        self.status_label.set_label(&msg);
        self.status_label.set_css_classes(&["success"]);
    }

    /// Restore persisted skip state on startup.
    /// Triggers the same visual update and on_skip_changed callback as a user click.
    pub fn set_skipped(&self, skipped: bool) {
        self.skip_checkbox.set_active(skipped);
    }

    /// Returns `Some(items)` when a non-empty proper subset of packages is
    /// selected for updating.
    ///
    /// Returns `None` when:
    /// - all packages are selected (full update, no filter needed), OR
    /// - the backend does not support item selection, OR
    /// - there are no packages loaded.
    pub fn items_to_update(&self) -> Option<Vec<String>> {
        if !self.supports_item_selection {
            return None;
        }
        let all = self.all_item_ids.borrow();
        let desel = self.deselected_items.borrow();
        if desel.is_empty() || all.is_empty() {
            return None; // All selected or nothing loaded.
        }
        if desel.len() >= all.len() {
            return None; // All deselected — backend is skipped; is_skipped() handles this.
        }
        let selected: Vec<String> = all
            .iter()
            .filter(|id| !desel.contains(*id))
            .cloned()
            .collect();
        if selected.is_empty() {
            None
        } else {
            Some(selected)
        }
    }
}
