use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::runner::CommandRunner;
use crate::ui::log_panel::LogPanel;
use crate::ui::update_row::UpdateRow;
use crate::ui::upgrade_page::UpgradePage;
use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

pub struct UpWindow;

impl UpWindow {
    pub fn build(app: &adw::Application) -> adw::ApplicationWindow {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Up")
            .default_width(700)
            .default_height(600)
            .build();

        let view_stack = adw::ViewStack::new();

        // --- Update Page ---
        let (update_page, run_checks) = Self::build_update_page();
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

        let refresh_button = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Check for updates")
            .build();
        let run_checks_btn = run_checks.clone();
        refresh_button.connect_clicked(move |_| (*run_checks_btn)());
        header.pack_start(&refresh_button);

        // Application overflow menu (three-dot button on the end/right slot).
        let app_menu = gio::Menu::new();
        app_menu.append(Some("About Up"), Some("win.about"));
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&app_menu)
            .tooltip_text("Main menu")
            .build();
        header.pack_end(&menu_button);

        let main_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        main_box.append(&header);
        main_box.append(&view_stack);
        main_box.append(&view_switcher_bar);

        window.set_content(Some(&main_box));

        // Register the "about" window action that opens the About dialog.
        let about_action = gio::SimpleAction::new("about", None);
        let window_ref = window.downgrade();
        about_action.connect_activate(move |_, _| {
            let Some(win) = window_ref.upgrade() else {
                return;
            };
            let dialog = adw::AboutDialog::builder()
                .application_name("Up")
                .version(env!("CARGO_PKG_VERSION"))
                .developer_name("Up Contributors")
                .comments("A system updater for Linux")
                .website("https://github.com/VictoryTek/Up")
                .application_icon("io.github.up")
                .license_type(gtk::License::Gpl30)
                .build();
            dialog.present(Some(&win));
        });
        window.add_action(&about_action);

        window
    }

    fn build_update_page() -> (gtk::Box, Rc<dyn Fn()>) {
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

        let detected: Rc<RefCell<Vec<Arc<dyn Backend>>>> = Rc::new(RefCell::new(Vec::new()));

        let rows: Rc<RefCell<Vec<(BackendKind, UpdateRow)>>> = Rc::new(RefCell::new(Vec::new()));

        // Placeholder row shown while background detection runs
        let placeholder_row = adw::ActionRow::builder()
            .title("Detecting package managers\u{2026}")
            .build();
        let placeholder_spinner = gtk::Spinner::new();
        placeholder_spinner.start();
        placeholder_row.add_suffix(&placeholder_spinner);
        backends_group.add(&placeholder_row);

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
            .sensitive(false)
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
            let backends = detected_clone.borrow().clone();

            glib::spawn_future_local(async move {
                let (tx, rx) = async_channel::unbounded::<(BackendKind, String)>();
                let (result_tx, result_rx) =
                    async_channel::unbounded::<(BackendKind, UpdateResult)>();

                // Clone senders for the worker thread
                let tx_thread = tx.clone();
                let result_tx_thread = result_tx.clone();
                let backends_thread = backends.clone();

                super::spawn_background_async(move || async move {
                    for backend in &backends_thread {
                        let kind = backend.kind();
                        let runner = CommandRunner::new(tx_thread.clone(), kind);
                        let result = backend.run_update(&runner).await;
                        let _ = result_tx_thread.send((kind, result)).await;
                    }
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

                if !has_error {
                    crate::ui::reboot_dialog::show_reboot_dialog(&button_ref);
                }
            });
        });

        content_box.append(&update_button);

        clamp.set_child(Some(&content_box));
        scrolled.set_child(Some(&clamp));
        page_box.append(&scrolled);

        // Shared state for gating the Update All button on check completion.
        let pending_checks: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
        let total_available: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
        let check_epoch: Rc<Cell<u64>> = Rc::new(Cell::new(0));

        // Build the availability-check closure. Shared with the header refresh button.
        let run_checks: Rc<dyn Fn()> = {
            let rows = rows.clone();
            let detected = detected.clone();
            let update_button_checks = update_button.clone();
            let pending_checks = pending_checks.clone();
            let total_available = total_available.clone();
            let check_epoch = check_epoch.clone();
            let status_label_checks = status_label.clone();
            Rc::new(move || {
                let n = detected.borrow().len();
                if n == 0 {
                    return;
                }
                // Disable button and reset counters at the start of each check cycle.
                update_button_checks.set_sensitive(false);
                *pending_checks.borrow_mut() = n;
                *total_available.borrow_mut() = 0;
                // Increment epoch to invalidate in-flight futures from the previous check.
                check_epoch.set(check_epoch.get() + 1);
                let my_epoch = check_epoch.get();
                status_label_checks.set_label("Checking for updates...");

                for (idx, backend) in detected.borrow().iter().enumerate() {
                    {
                        let borrowed = rows.borrow();
                        borrowed[idx].1.set_status_checking();
                    }
                    let backend_clone = backend.clone();
                    let rows_ref = rows.clone();
                    let pending_ref = pending_checks.clone();
                    let total_ref = total_available.clone();
                    let btn_ref = update_button_checks.clone();
                    let status_ref = status_label_checks.clone();
                    let epoch_ref = check_epoch.clone();
                    glib::spawn_future_local(async move {
                        type CheckPayload = (Result<usize, String>, Result<Vec<String>, String>);
                        let (tx, rx) = async_channel::bounded::<CheckPayload>(1);
                        super::spawn_background_async(move || async move {
                            let count = backend_clone.count_available().await;
                            let list = backend_clone.list_available().await;
                            let _ = tx.send((count, list)).await;
                        });
                        if let Ok((count_result, list_result)) = rx.recv().await {
                            // Discard results from a superseded check cycle.
                            if epoch_ref.get() != my_epoch {
                                return;
                            }
                            let row = rows_ref.borrow()[idx].1.clone();
                            match count_result {
                                Ok(count) => {
                                    row.set_status_available(count);
                                    *total_ref.borrow_mut() += count;
                                }
                                Err(msg) => {
                                    row.set_status_unknown(&msg);
                                }
                            }
                            match list_result {
                                Ok(packages) => row.set_packages(&packages),
                                Err(_) => row.set_packages(&[]),
                            }
                            let remaining = {
                                let mut p = pending_ref.borrow_mut();
                                *p -= 1;
                                *p
                            };
                            if remaining == 0 {
                                let total = *total_ref.borrow();
                                if total > 0 {
                                    btn_ref.set_sensitive(true);
                                    status_ref.set_label(&format!(
                                        "{total} update{} available",
                                        if total == 1 { "" } else { "s" }
                                    ));
                                } else {
                                    status_ref.set_label("Everything is up to date.");
                                }
                            }
                        }
                    });
                }
            })
        };

        // Spawn backend detection off the GTK thread.
        {
            let (detect_tx, detect_rx) = async_channel::unbounded::<Vec<Arc<dyn Backend>>>();

            let detected_fill = detected.clone();
            let rows_fill = rows.clone();
            let group_fill = backends_group.clone();
            let run_checks_after_detect = run_checks.clone();

            super::spawn_background_async(move || async move {
                let backends = crate::backends::detect_backends();
                let _ = detect_tx.send(backends).await;
            });

            glib::spawn_future_local(async move {
                if let Ok(new_backends) = detect_rx.recv().await {
                    // Remove placeholder
                    group_fill.remove(&placeholder_row);
                    // Populate rows
                    {
                        let mut rows_mut = rows_fill.borrow_mut();
                        for backend in &new_backends {
                            let row = UpdateRow::new(backend.as_ref());
                            group_fill.add(&row.row);
                            rows_mut.push((backend.kind(), row));
                        }
                    }
                    // Store backends
                    *detected_fill.borrow_mut() = new_backends;
                    // Trigger availability check (enables Update All only if updates are found)
                    (*run_checks_after_detect)();
                } else {
                    eprintln!("Backend detection failed; no backends detected.");
                    group_fill.remove(&placeholder_row);
                }
            });
        }

        (page_box, run_checks)
    }
}
