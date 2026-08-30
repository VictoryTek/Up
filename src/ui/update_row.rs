use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::backends::{Backend, BackendKind};

#[derive(Clone)]
pub struct UpdateRow {
    pub row: adw::ActionRow,
    status_label: gtk::Label,
    spinner: gtk::Spinner,
    /// This backend's kind, for changelog fetching.
    kind: BackendKind,
    /// "What's new" button; shown only when updates are pending and the
    /// backend supports changelog fetching.
    changelog_button: gtk::Button,
    /// Package names from the most recent `set_packages()` call, passed to the
    /// changelog fetcher.
    packages: Rc<RefCell<Vec<String>>>,
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
    /// Estimated additional disk space (bytes) the pending updates need, from
    /// the most recent check. `None` when the backend cannot estimate.
    last_estimated_size: Rc<Cell<Option<u64>>>,
    /// Set when the most recent check returned an error; reset when a new check starts.
    /// Lets the window distinguish "0 updates confirmed" from "check failed".
    check_errored: Rc<Cell<bool>>,
    skip_checkbox: gtk::CheckButton,
    retry_button: gtk::Button,
    /// Whether the backend supports updating a user-selected subset of packages.
    supports_selection: bool,
    /// One `(item_id, checkbox)` pair per currently-displayed package.
    package_checks: Rc<RefCell<Vec<(String, gtk::CheckButton)>>>,
    /// `true` when the last `set_packages()` call exceeded the display cap,
    /// meaning the full list isn't representable in the UI and selection
    /// can't be trusted to reflect the user's actual intent.
    selection_capped: Rc<Cell<bool>>,
}

impl UpdateRow {
    pub fn new(
        backend: &dyn Backend,
        initial_skipped: bool,
        on_skip_changed: impl Fn() + 'static,
        on_retry: impl Fn() + 'static,
    ) -> Self {
        let status_label = gtk::Label::builder()
            .label(if initial_skipped { "Skipped" } else { "Ready" })
            .css_classes(vec!["dim-label"])
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();

        let spinner = gtk::Spinner::builder().visible(false).build();

        let icon = gtk::Image::builder()
            .icon_name(backend.icon_name())
            .accessible_role(gtk::AccessibleRole::Presentation)
            .build();

        let skip_flag = Rc::new(Cell::new(initial_skipped));
        let last_available: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));
        let supports_selection = backend.supports_item_selection();
        let package_checks: Rc<RefCell<Vec<(String, gtk::CheckButton)>>> =
            Rc::new(RefCell::new(Vec::new()));
        let selection_capped = Rc::new(Cell::new(false));

        let kind_label = format!("Skip {} during Update All", backend.display_name());
        let skip_checkbox = gtk::CheckButton::builder()
            .tooltip_text(&kind_label)
            .valign(gtk::Align::Center)
            .active(initial_skipped)
            .build();
        skip_checkbox.update_property(&[gtk::accessible::Property::Label(kind_label.as_str())]);

        let backend_name = backend.display_name().to_string();
        let kind = backend.kind();

        let row = adw::ActionRow::builder()
            .title(backend.display_name())
            .subtitle(backend.description())
            .build();

        let retry_button = gtk::Button::from_icon_name("view-refresh-symbolic");
        retry_button.set_tooltip_text(Some("Retry"));
        retry_button.set_visible(false);
        retry_button.connect_clicked(move |_| on_retry());

        let packages: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

        let changelog_button = gtk::Button::from_icon_name("document-properties-symbolic");
        changelog_button.set_tooltip_text(Some("What's new"));
        changelog_button.set_valign(gtk::Align::Center);
        changelog_button.add_css_class("flat");
        changelog_button.set_visible(false);
        {
            let kind = kind.clone();
            let packages = packages.clone();
            let row = row.clone();
            changelog_button.connect_clicked(move |_| {
                show_changelog_dialog(&row, kind.clone(), packages.borrow().clone());
            });
        }

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
        row.add_suffix(&changelog_button);
        row.add_suffix(&retry_button);
        row.add_suffix(&skip_checkbox);

        {
            let skip_flag = skip_flag.clone();
            let last_available = last_available.clone();
            let status_label = status_label.clone();
            let changelog_button = changelog_button.clone();
            let kind = kind.clone();
            skip_checkbox.connect_toggled(move |cb| {
                let skipped = cb.is_active();
                skip_flag.set(skipped);
                if skipped {
                    status_label.set_label("Skipped");
                    status_label.set_css_classes(&["dim-label"]);
                    changelog_button.set_visible(false);
                } else {
                    changelog_button.set_visible(
                        matches!(last_available.get(), Some(c) if c > 0)
                            && crate::changelog::supports_changelog(&kind),
                    );
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
            kind,
            changelog_button,
            packages,
            menu_button,
            popover_heading,
            popover_list,
            backend_name,
            skip_flag,
            last_available,
            last_estimated_size: Rc::new(Cell::new(None)),
            check_errored: Rc::new(Cell::new(false)),
            skip_checkbox,
            retry_button,
            supports_selection,
            package_checks,
            selection_capped,
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

    /// Records the estimated download/disk size (bytes) for this backend's
    /// pending updates. `None` clears it (backend can't estimate).
    pub fn set_estimated_size(&self, bytes: Option<u64>) {
        self.last_estimated_size.set(bytes);
    }

    /// Returns the last estimated size (bytes) for this backend's pending
    /// updates, or `None` when unavailable.
    pub fn last_estimated_size(&self) -> Option<u64> {
        self.last_estimated_size.get()
    }

    /// Returns `true` if the most recent check produced an error.
    /// Reset to `false` at the start of each check cycle via `set_status_checking()`.
    pub fn has_check_error(&self) -> bool {
        self.check_errored.get()
    }

    /// Populate the popover with a list of pending package names, each with
    /// a selection checkbox. Clears any previously added rows before adding
    /// new ones. Caps display at 50 items with a summary row for the
    /// remainder.
    ///
    /// Checkboxes are only interactive when the backend supports selective
    /// updates *and* every package fits within the display cap — if the
    /// list is truncated, some packages have no checkbox to deselect, so
    /// showing a partial selection as meaningful would misrepresent what
    /// `run_selected_update` would actually receive.
    pub fn set_packages(&self, packages: &[String]) {
        *self.packages.borrow_mut() = packages.to_vec();
        // Remove previously added package rows to avoid duplicates on re-check.
        while let Some(child) = self.popover_list.first_child() {
            self.popover_list.remove(&child);
        }
        self.package_checks.borrow_mut().clear();

        // Hide the pill button when there is nothing to show.
        if packages.is_empty() {
            self.menu_button.set_visible(false);
            self.selection_capped.set(false);
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
        let capped = packages.len() > MAX_PACKAGES;
        self.selection_capped.set(capped);
        let interactive = self.supports_selection && !capped;

        let display_count = packages.len().min(MAX_PACKAGES);
        let mut checks = self.package_checks.borrow_mut();
        for pkg in &packages[..display_count] {
            let item_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let check = gtk::CheckButton::builder()
                .active(true)
                .sensitive(interactive)
                .valign(gtk::Align::Center)
                .build();
            check.update_property(&[gtk::accessible::Property::Label(pkg.as_str())]);
            let label = gtk::Label::builder()
                .label(pkg.as_str())
                .halign(gtk::Align::Start)
                .hexpand(true)
                .build();
            item_row.append(&check);
            item_row.append(&label);
            self.popover_list.append(&item_row);
            checks.push((pkg.clone(), check));
        }
        drop(checks);

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

    /// Returns the subset of packages the user has selected for update, or
    /// `None` when selection doesn't meaningfully apply: the backend
    /// doesn't support it, the package list exceeds the display cap, no
    /// list has been loaded yet, or every checkbox is checked (identical
    /// to a full update).
    ///
    /// May return `Some(vec![])` if the user unchecked every package —
    /// callers must treat that as "nothing to do for this backend", never
    /// forward it to `run_selected_update` (which falls back to a *full*
    /// update on an empty slice).
    pub fn selected_items(&self) -> Option<Vec<String>> {
        if !self.supports_selection || self.selection_capped.get() {
            return None;
        }
        let checks = self.package_checks.borrow();
        if checks.is_empty() {
            return None;
        }
        let selected: Vec<String> = checks
            .iter()
            .filter(|(_, cb)| cb.is_active())
            .map(|(id, _)| id.clone())
            .collect();
        if selected.len() == checks.len() {
            None
        } else {
            Some(selected)
        }
    }

    pub fn set_status_checking(&self) {
        self.retry_button.set_visible(false);
        self.changelog_button.set_visible(false);
        self.last_available.set(None);
        self.last_estimated_size.set(None);
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
        self.changelog_button
            .set_visible(count > 0 && crate::changelog::supports_changelog(&self.kind));
    }

    pub fn set_status_running(&self) {
        self.retry_button.set_visible(false);
        self.changelog_button.set_visible(false);
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
        self.changelog_button.set_visible(false);
        self.skip_checkbox.set_sensitive(true);
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.check_errored.set(true);
        self.status_label.set_label(msg);
        self.status_label.set_css_classes(&["dim-label"]);
    }
}

/// Present a "What's new" dialog anchored at `parent` and asynchronously fill
/// it with `fetch_changelog` output for the given backend and pending packages.
fn show_changelog_dialog(
    parent: &impl gtk::prelude::IsA<gtk::Widget>,
    kind: crate::backends::BackendKind,
    packages: Vec<String>,
) {
    let text_view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .left_margin(8)
        .right_margin(8)
        .top_margin(8)
        .bottom_margin(8)
        .build();
    text_view.buffer().set_text("Fetching changelog\u{2026}");

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(320)
        .min_content_width(480)
        .child(&text_view)
        .build();

    let dialog = adw::AlertDialog::builder()
        .heading("What's New")
        .extra_child(&scroller)
        .build();
    dialog.add_response("close", "Close");
    dialog.set_default_response(Some("close"));
    dialog.set_close_response("close");
    dialog.present(Some(parent));

    let (tx, rx) = async_channel::bounded::<Result<String, String>>(1);
    super::spawn_background_async(move || async move {
        let result = crate::changelog::fetch_changelog(kind, &packages)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(result).await;
    });
    glib::spawn_future_local(async move {
        let text = match rx.recv().await {
            Ok(Ok(s)) if s.trim().is_empty() => "No changelog information available.".to_string(),
            Ok(Ok(s)) => s,
            Ok(Err(e)) => format!("Could not fetch changelog:\n{e}"),
            Err(_) => "Could not fetch changelog.".to_string(),
        };
        text_view.buffer().set_text(&text);
    });
}
