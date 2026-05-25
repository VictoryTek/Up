#![allow(dead_code)]
use adw::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

pub struct HistoryPage;

impl HistoryPage {
    /// Build the History page widget.
    ///
    /// Returns the root `gtk::Box` widget to be added to the ViewStack.
    pub fn build() -> gtk::Box {
        let page_box = gtk::Box::new(gtk::Orientation::Vertical, 0);

        let scrolled = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();

        let clamp = adw::Clamp::builder()
            .maximum_size(600)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .build();

        let content_box = gtk::Box::new(gtk::Orientation::Vertical, 18);

        // Header description
        let header_label = gtk::Label::builder()
            .label(gettext("A record of past update sessions."))
            .css_classes(vec!["dim-label"])
            .build();
        content_box.append(&header_label);

        // History group
        let history_group = adw::PreferencesGroup::builder()
            .title(gettext("Update History"))
            .build();

        // Clear button in PreferencesGroup header
        let clear_button = gtk::Button::builder()
            .label(gettext("Clear"))
            .css_classes(vec!["destructive-action"])
            .valign(gtk::Align::Center)
            .build();
        clear_button.update_property(&[gtk::accessible::Property::Label(&gettext(
            "Clear update history",
        ))]);
        history_group.set_header_suffix(Some(&clear_button));

        // Track rows for clearing
        let tracked_rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::new(RefCell::new(Vec::new()));

        // Populate from disk
        Self::populate(&history_group, &tracked_rows);

        // Wire up clear button
        {
            let history_group_weak = history_group.downgrade();
            let tracked_rows_clone = tracked_rows.clone();
            clear_button.connect_clicked(move |_| {
                let _ = crate::history::clear_history();
                if let Some(group) = history_group_weak.upgrade() {
                    {
                        let mut tracked = tracked_rows_clone.borrow_mut();
                        for row in tracked.drain(..) {
                            group.remove(&row);
                        }
                    }
                    Self::populate(&group, &tracked_rows_clone);
                }
            });
        }

        content_box.append(&history_group);
        clamp.set_child(Some(&content_box));
        scrolled.set_child(Some(&clamp));
        page_box.append(&scrolled);
        page_box
    }

    /// Populate `group` with rows from the history file.
    fn populate(group: &adw::PreferencesGroup, tracked_rows: &Rc<RefCell<Vec<adw::ActionRow>>>) {
        let entries = crate::history::load_entries().unwrap_or_default();

        if entries.is_empty() {
            let empty_row = adw::ActionRow::builder()
                .title(gettext("No history yet"))
                .subtitle(gettext(
                    "Update sessions will appear here after you run an update.",
                ))
                .build();
            group.add(&empty_row);
            tracked_rows.borrow_mut().push(empty_row);
            return;
        }

        // Show newest first
        for entry in entries.iter().rev() {
            let timestamp_str = format_timestamp(entry.timestamp);
            let subtitle = match entry.result.as_str() {
                "success" | "success_self_update" => match entry.updated_count {
                    Some(n) if n > 0 => format!("{timestamp_str} \u{2014} {n} updated"),
                    _ => format!("{timestamp_str} \u{2014} up to date"),
                },
                "error" => format!(
                    "{timestamp_str} \u{2014} {}",
                    entry.error.as_deref().unwrap_or("unknown error")
                ),
                "skipped" => format!("{timestamp_str} \u{2014} skipped"),
                _ => timestamp_str,
            };

            let row = adw::ActionRow::builder()
                .title(&entry.backend)
                .subtitle(&subtitle)
                .build();

            let icon_name = match entry.result.as_str() {
                "success" | "success_self_update" => "emblem-ok-symbolic",
                "error" => "dialog-error-symbolic",
                "skipped" => "action-unavailable-symbolic",
                _ => "dialog-question-symbolic",
            };
            let icon = gtk::Image::from_icon_name(icon_name);
            icon.set_accessible_role(gtk::AccessibleRole::Presentation);
            row.add_prefix(&icon);

            group.add(&row);
            tracked_rows.borrow_mut().push(row);
        }
    }
}

/// Format a Unix timestamp as a human-readable local date/time string.
///
/// Uses `glib::DateTime` for local-timezone formatting. Format: "YYYY-MM-DD HH:MM".
fn format_timestamp(secs: u64) -> String {
    glib::DateTime::from_unix_local(secs as i64)
        .ok()
        .and_then(|dt| dt.format("%Y-%m-%d %H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| secs.to_string())
}
