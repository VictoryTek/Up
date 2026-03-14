use adw::prelude::*;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

use crate::backends::{self, Backend, BackendKind, UpdateResult};
use crate::runner::CommandRunner;
use crate::ui::log_panel::LogPanel;
use crate::ui::update_row::UpdateRow;
use crate::ui::upgrade_page::UpgradePage;

pub struct UpWindow {
    pub window: adw::ApplicationWindow,
}

impl UpWindow {
    pub fn new(app: &adw::Application) -> adw::ApplicationWindow {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Up")
            .default_width(700)
            .default_height(600)
            .build();

        let view_stack = adw::ViewStack::new();

        // --- Update Page ---
        let update_page = Self::build_update_page();
        view_stack.add_titled_with_icon(
            &update_page,
            Some("update"),
            "Update",
            "software-update-available-symbolic",
        );

        // --- Upgrade Page ---
        let upgrade_page = UpgradePage::build();
        view_stack.add_titled_with_icon(
            &upgrade_page,
            Some("upgrade"),
            "Upgrade",
            "software-update-urgent-symbolic",
        );

        let view_switcher_bar = adw::ViewSwitcherBar::builder()
            .stack(&view_stack)
            .reveal(true)
            .build();

        let header = adw::HeaderBar::new();
        let view_switcher_title = adw::ViewSwitcherTitle::builder()
            .stack(&view_stack)
            .title("Up")
            .build();
        header.set_title_widget(Some(&view_switcher_title));

        view_switcher_title.connect_title_visible_notify({
            let bar = view_switcher_bar.clone();
            move |switcher| {
                bar.set_reveal(switcher.is_title_visible());
            }
        });

        let main_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        main_box.append(&header);
        main_box.append(&view_stack);
        main_box.append(&view_switcher_bar);

        window.set_content(Some(&main_box));
        window
    }

    fn build_update_page() -> gtk::Box {
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

        // Status label
        let status_label = gtk::Label::builder()
            .label("Detect available updates across your system.")
            .css_classes(vec!["dim-label"])
            .build();
        content_box.append(&status_label);

        // Backend rows group
        let backends_group = adw::PreferencesGroup::builder()
            .title("Sources")
            .description("Package managers detected on this system")
            .build();

        let detected = backends::detect_backends();

        let rows: Rc<RefCell<Vec<(BackendKind, UpdateRow)>>> = Rc::new(RefCell::new(Vec::new()));

        for backend in &detected {
            let row = UpdateRow::new(backend);
            backends_group.add(&row.row);
            rows.borrow_mut().push((backend.kind(), row));
        }

        content_box.append(&backends_group);

        // Log panel (expandable terminal output)
        let log_panel = LogPanel::new();
        content_box.append(&log_panel.expander);

        // Update All button
        let update_button = gtk::Button::builder()
            .label("Update All")
            .css_classes(vec!["suggested-action", "pill"])
            .halign(gtk::Align::Center)
            .margin_top(12)
            .build();

        let status_clone = status_label.clone();
        let rows_clone = rows.clone();
        let log_clone = log_panel.clone();
        let detected_clone = detected.clone();

        update_button.connect_clicked(move |button| {
            button.set_sensitive(false);
            status_clone.set_label("Updating...");
            log_clone.clear();

            let rows_ref = rows_clone.clone();
            let log_ref = log_clone.clone();
            let status_ref = status_clone.clone();
            let button_ref = button.clone();
            let backends = detected_clone.clone();

            glib::spawn_future_local(async move {
                let (tx, rx) = async_channel::unbounded::<(BackendKind, String)>();
                let (result_tx, result_rx) =
                    async_channel::unbounded::<(BackendKind, UpdateResult)>();

                // Spawn blocking update work on a thread
                let tx_clone = tx.clone();
                let result_tx_clone = result_tx.clone();
                let backends_thread = backends.clone();

                std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();

                    rt.block_on(async {
                        for backend in &backends_thread {
                            let kind = backend.kind();
                            let runner = CommandRunner::new(tx_clone.clone(), kind);
                            let result = backend.run_update(&runner).await;
                            let _ = result_tx_clone.send((kind, result)).await;
                        }
                    });

                    drop(tx_clone);
                    drop(result_tx_clone);
                });

                // Process log output
                let log_ref2 = log_ref.clone();
                let rows_ref2 = rows_ref.clone();

                glib::spawn_future_local(async move {
                    while let Ok((kind, line)) = rx.recv().await {
                        log_ref2.append_line(&format!("[{kind}] {line}"));
                    }
                });

                // Process results
                while let Ok((kind, result)) = result_rx.recv().await {
                    let rows_borrowed = rows_ref.borrow();
                    if let Some((_, row)) = rows_borrowed.iter().find(|(k, _)| *k == kind) {
                        match &result {
                            UpdateResult::Success { updated_count } => {
                                row.set_status_success(*updated_count);
                            }
                            UpdateResult::Error(msg) => {
                                row.set_status_error(msg);
                            }
                            UpdateResult::Skipped(msg) => {
                                row.set_status_skipped(msg);
                            }
                        }
                    }
                }

                status_ref.set_label("Update complete.");
                button_ref.set_sensitive(true);
            });
        });

        content_box.append(&update_button);

        clamp.set_child(Some(&content_box));
        scrolled.set_child(Some(&clamp));
        page_box.append(&scrolled);

        page_box
    }
}
