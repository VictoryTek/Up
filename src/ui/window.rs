use crate::backends::{self, BackendKind, UpdateResult};
use crate::runner::CommandRunner;
use crate::ui::log_panel::LogPanel;
use crate::ui::update_row::UpdateRow;
use crate::ui::upgrade_page::UpgradePage;
use adw::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

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
            let row = UpdateRow::new(backend.as_ref());
            backends_group.add(&row.row);
            rows.borrow_mut().push((backend.kind(), row));
        }

        // Auto-check for available updates on launch
        for (idx, backend) in detected.iter().enumerate() {
            {
                let borrowed = rows.borrow();
                borrowed[idx].1.set_status_checking();
            }

            let backend_clone = backend.clone();
            let rows_ref = rows.clone();

            glib::spawn_future_local(async move {
                let (tx, rx) = async_channel::bounded::<Result<usize, String>>(1);

                std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    rt.block_on(async {
                        let result = backend_clone.count_available().await;
                        let _ = tx.send(result).await;
                    });
                });

                if let Ok(result) = rx.recv().await {
                    let row = rows_ref.borrow()[idx].1.clone();
                    match result {
                        Ok(count) => row.set_status_available(count),
                        Err(_) => row.set_status_available(0),
                    }
                }
            });
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

            // Set all rows to "Updating..." state
            {
                let rows_borrowed = rows_clone.borrow();
                for (_, row) in rows_borrowed.iter() {
                    row.set_status_running();
                }
            }

            let rows_ref = rows_clone.clone();
            let log_ref = log_clone.clone();
            let status_ref = status_clone.clone();
            let button_ref = button.clone();
            let backends = detected_clone.clone();

            glib::spawn_future_local(async move {
                let (tx, rx) = async_channel::unbounded::<(BackendKind, String)>();
                let (result_tx, result_rx) =
                    async_channel::unbounded::<(BackendKind, UpdateResult)>();

                // Clone senders for the worker thread
                let tx_thread = tx.clone();
                let result_tx_thread = result_tx.clone();
                let backends_thread = backends.clone();

                std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();

                    rt.block_on(async {
                        for backend in &backends_thread {
                            let kind = backend.kind();
                            let runner = CommandRunner::new(tx_thread.clone(), kind);
                            let result = backend.run_update(&runner).await;
                            let _ = result_tx_thread.send((kind, result)).await;
                        }
                    });

                    drop(tx_thread);
                    drop(result_tx_thread);
                });

                // Drop the original senders so channels close when the thread finishes
                drop(tx);
                drop(result_tx);

                // Process log output in a separate future
                let log_ref2 = log_ref.clone();
                let rows_for_log = rows_ref.clone();
                glib::spawn_future_local(async move {
                    while let Ok((kind, line)) = rx.recv().await {
                        log_ref2.append_line(&format!("[{kind}] {line}"));
                        let borrowed = rows_for_log.borrow();
                        if let Some((_, row)) = borrowed.iter().find(|(k, _)| *k == kind) {
                            row.pulse_progress();
                        }
                    }
                });

                // Process results
                let mut has_error = false;
                while let Ok((kind, result)) = result_rx.recv().await {
                    let rows_borrowed = rows_ref.borrow();
                    if let Some((_, row)) = rows_borrowed.iter().find(|(k, _)| *k == kind) {
                        match &result {
                            UpdateResult::Success { updated_count } => {
                                row.set_status_success(*updated_count);
                            }
                            UpdateResult::Error(msg) => {
                                row.set_status_error(msg);
                                has_error = true;
                            }
                            UpdateResult::Skipped(msg) => {
                                row.set_status_skipped(msg);
                            }
                        }
                    }
                }

                if has_error {
                    status_ref.set_label("Update completed with errors.");
                } else {
                    status_ref.set_label("Update complete.");
                }
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
