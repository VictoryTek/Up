use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::runner::{BackendEvent, CommandRunner, PrivilegedShell};
use crate::ui::log_panel::LogPanel;
use crate::ui::update_row::UpdateRow;
use crate::ui::upgrade_page::UpgradePage;
use crate::upgrade;
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
        let (update_page, run_checks, sysinfo_distro_row, sysinfo_version_row) =
            Self::build_update_page();
        view_stack.add_titled_with_icon(
            &update_page,
            Some("update"),
            "Update",
            "software-update-available-symbolic",
        );

        // --- Upgrade Page ---
        let (upgrade_widget, upgrade_init_tx) = UpgradePage::build();
        let upgrade_stack_page = view_stack.add_titled_with_icon(
            &upgrade_widget,
            Some("upgrade"),
            "Upgrade",
            "software-update-urgent-symbolic",
        );

        let view_switcher_bar = adw::ViewSwitcherBar::builder()
            .stack(&view_stack)
            .reveal(true)
            .build();

        // Spawn single distro detection, fanning out to update-page sysinfo and upgrade page.
        {
            let (detect_tx, detect_rx) = async_channel::bounded::<(
                upgrade::DistroInfo,
                Option<(upgrade::NixOsConfigType, String)>,
            )>(1);

            super::spawn_background_async(move || async move {
                let info = upgrade::detect_distro();
                let nixos_extra = if info.id == "nixos" {
                    let config_type = upgrade::detect_nixos_config_type();
                    let raw_hostname = upgrade::detect_hostname();
                    Some((config_type, raw_hostname))
                } else {
                    None
                };
                let _ = detect_tx.send((info, nixos_extra)).await;
            });

            let view_switcher_bar = view_switcher_bar.clone();
            glib::spawn_future_local(async move {
                if let Ok((info, nixos_extra)) = detect_rx.recv().await {
                    // 1. Populate update-page system info rows
                    sysinfo_distro_row.set_subtitle(&info.name);
                    sysinfo_version_row.set_subtitle(&info.version);

                    // 2. Gate upgrade tab visibility — hide for unsupported distros.
                    if !info.upgrade_supported {
                        upgrade_stack_page.set_visible(false);
                        view_switcher_bar.set_reveal(false);
                    }

                    // 3. Forward to upgrade page
                    if info.upgrade_supported {
                        let init = upgrade::UpgradePageInit {
                            distro: info,
                            nixos_extra,
                        };
                        let _ = upgrade_init_tx.send(init).await;
                    }
                }
            });
        }

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

    fn build_update_page() -> (gtk::Box, Rc<dyn Fn()>, adw::ActionRow, adw::ActionRow) {
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

        // System Information group (populated after background distro detection)
        let sys_info_group = adw::PreferencesGroup::builder()
            .title("System Information")
            .build();

        let distro_row = adw::ActionRow::builder()
            .title("Distribution")
            .subtitle("Loading\u{2026}")
            .build();
        distro_row.add_prefix(&gtk::Image::from_icon_name("computer-symbolic"));
        sys_info_group.add(&distro_row);

        let version_row = adw::ActionRow::builder()
            .title("Current Version")
            .subtitle("Loading\u{2026}")
            .build();
        sys_info_group.add(&version_row);

        content_box.append(&sys_info_group);

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

        // Restart notification banner, revealed only when Up itself is updated
        // inside the Flatpak sandbox (new deployment is available on next launch).
        let restart_banner = adw::Banner::builder()
            .title("Up was updated \u{2014} restart to apply changes")
            .button_label("Close Up")
            .revealed(false)
            .build();
        restart_banner.connect_button_clicked(|banner| {
            if let Some(window) = banner.root().and_then(|r| r.downcast::<gtk::Window>().ok()) {
                window.close();
            }
        });

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
        let restart_banner_clone = restart_banner.clone();

        update_button.connect_clicked(move |button| {
            button.set_sensitive(false);
            log_clone.clear();

            let rows_ref = rows_clone.clone();
            let log_ref = log_clone.clone();
            let status_ref = status_clone.clone();
            let button_ref = button.clone();
            let backends = detected_clone.borrow().clone();
            let banner_ref = restart_banner_clone.clone();

            // Check if any backend requires root privileges.
            let any_needs_root = backends.iter().any(|b| b.needs_root());

            glib::spawn_future_local(async move {
                if any_needs_root {
                    status_ref.set_label("Authenticating\u{2026}");
                    log_ref.append_line("Requesting administrator privileges\u{2026}");
                }

                // Reorder: privileged backends first, then unprivileged.
                // This maximises the polkit credential cache benefit.
                let mut ordered_backends = backends.clone();
                ordered_backends.sort_by_key(|b| u8::from(!b.needs_root()));

                let (event_tx, event_rx) = async_channel::unbounded::<BackendEvent>();

                // Auth status: Ok(()) = authenticated (or not needed), Err(msg) = failed.
                let (auth_status_tx, auth_status_rx) =
                    async_channel::bounded::<Result<(), String>>(1);

                let event_tx_thread = event_tx.clone();

                // IMPORTANT: PrivilegedShell must be created AND used within the
                // same Tokio runtime.  Its Tokio I/O handles (ChildStdin/ChildStdout)
                // are bound to the reactor of the runtime that created them.  Passing
                // them across spawn_background_async boundaries (each of which builds
                // its own runtime) causes write operations to fail because the original
                // reactor is no longer running.
                super::spawn_background_async(move || async move {
                    let shell: Option<Arc<tokio::sync::Mutex<PrivilegedShell>>> = if any_needs_root
                    {
                        match PrivilegedShell::new().await {
                            Ok(s) => {
                                let _ = auth_status_tx.send(Ok(())).await;
                                Some(Arc::new(tokio::sync::Mutex::new(s)))
                            }
                            Err(e) => {
                                let _ = auth_status_tx.send(Err(e)).await;
                                return;
                            }
                        }
                    } else {
                        let _ = auth_status_tx.send(Ok(())).await;
                        None
                    };

                    for backend in &ordered_backends {
                        let kind = backend.kind();
                        let _ = event_tx_thread.send(BackendEvent::Started(kind)).await;
                        let runner =
                            CommandRunner::new(event_tx_thread.clone(), kind, shell.clone());
                        let result = backend.run_update(&runner).await;
                        let _ = event_tx_thread
                            .send(BackendEvent::Finished(kind, result))
                            .await;
                    }
                    // Shut down the privileged shell now that all backends are done.
                    if let Some(s) = shell {
                        s.lock().await.close().await;
                    }
                });

                // Drop the original sender so the channel closes when the thread finishes.
                drop(event_tx);

                // Wait for authentication result before updating the UI.
                match auth_status_rx.recv().await {
                    Ok(Ok(())) => {
                        if any_needs_root {
                            log_ref.append_line("Authentication successful.");
                        }
                    }
                    Ok(Err(e)) => {
                        log_ref.append_line(&format!("Authentication failed: {e}"));
                        status_ref.set_label("Update cancelled.");
                        button_ref.set_sensitive(true);
                        return;
                    }
                    Err(_) => {
                        log_ref.append_line("Authentication channel closed unexpectedly.");
                        status_ref.set_label("Update cancelled.");
                        button_ref.set_sensitive(true);
                        return;
                    }
                }

                // --- Begin updates ---
                status_ref.set_label("Updating\u{2026}");

                // Process all backend events in strict arrival order via a single loop.
                // This ensures Started(A) → LogLine(A)* → Finished(A) → Started(B) …
                // are always processed in sequence, eliminating the three-channel race.
                let mut has_error = false;
                let mut self_updated = false;
                while let Ok(event) = event_rx.recv().await {
                    match event {
                        BackendEvent::Started(kind) => {
                            let rows_borrowed = rows_ref.borrow();
                            if let Some((_, row)) = rows_borrowed.iter().find(|(k, _)| *k == kind) {
                                row.set_status_running();
                            }
                        }
                        BackendEvent::LogLine(kind, line) => {
                            log_ref.append_line(&format!("[{kind}] {line}"));
                        }
                        BackendEvent::Finished(kind, result) => {
                            let rows_borrowed = rows_ref.borrow();
                            if let Some((_, row)) = rows_borrowed.iter().find(|(k, _)| *k == kind) {
                                match &result {
                                    UpdateResult::Success { updated_count } => {
                                        row.set_status_success(*updated_count);
                                    }
                                    UpdateResult::SuccessWithSelfUpdate { updated_count } => {
                                        row.set_status_success(*updated_count);
                                        self_updated = true;
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
                    }
                }

                if self_updated {
                    banner_ref.set_revealed(true);
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
        page_box.append(&restart_banner);
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

        (page_box, run_checks, distro_row, version_row)
    }
}
