use crate::backends::{Backend, BackendKind, UpdateResult};
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

type UpdatePageResult = (
    gtk::Box,
    Rc<dyn Fn()>,
    adw::ActionRow,
    adw::ActionRow,
    Rc<Cell<bool>>,
);

pub struct UpWindow;

impl UpWindow {
    pub fn build(app: &adw::Application) -> adw::ApplicationWindow {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Up")
            .default_width(760)
            .default_height(730)
            .build();
        window.add_css_class("up-window");

        let view_stack = adw::ViewStack::new();

        // --- Update Page ---
        let (update_page, run_checks, sysinfo_distro_row, sysinfo_version_row, update_in_progress) =
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

        // ViewSwitcher lives in the header bar center slot (hidden when only one tab is visible).
        let view_switcher = adw::ViewSwitcher::builder()
            .stack(&view_stack)
            .policy(adw::ViewSwitcherPolicy::Wide)
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

            let view_switcher_async = view_switcher.clone();
            glib::spawn_future_local(async move {
                if let Ok((info, nixos_extra)) = detect_rx.recv().await {
                    // 1. Populate update-page system info rows
                    sysinfo_distro_row.set_subtitle(&info.name);
                    sysinfo_version_row.set_subtitle(&info.version);

                    // 2. Gate upgrade tab visibility — hide for unsupported distros.
                    if !info.upgrade_supported {
                        upgrade_stack_page.set_visible(false);
                        view_switcher_async.set_visible(false);
                    } else {
                        view_switcher_async.set_visible(true);
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
        header.set_title_widget(Some(&view_switcher));

        let refresh_button = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Check for updates")
            .build();
        refresh_button.update_property(&[gtk::accessible::Property::Label("Refresh update list")]);
        refresh_button.connect_clicked(glib::clone!(
            #[strong]
            run_checks,
            #[strong]
            update_in_progress,
            move |_| {
                if update_in_progress.get() {
                    return;
                }
                (*run_checks)()
            }
        ));
        header.pack_start(&refresh_button);

        // Application overflow menu (three-dot button on the end/right slot).
        let app_menu = gio::Menu::new();
        app_menu.append(Some("About Up"), Some("win.about"));
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&app_menu)
            .tooltip_text("Main menu")
            .build();
        menu_button.update_property(&[gtk::accessible::Property::Label("Application menu")]);
        header.pack_end(&menu_button);

        let main_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        main_box.append(&header);
        main_box.append(&view_stack);

        window.set_content(Some(&main_box));

        // Register the "about" window action that opens the About dialog.
        let about_action = gio::SimpleAction::new("about", None);
        about_action.connect_activate(glib::clone!(
            #[weak]
            window,
            #[upgrade_or]
            return,
            move |_, _| {
                let dialog = adw::AboutDialog::builder()
                    .application_name("Up")
                    .version(env!("CARGO_PKG_VERSION"))
                    .developer_name("Up Contributors")
                    .comments("A system updater for Linux")
                    .website("https://github.com/VictoryTek/Up")
                    .application_icon("io.github.up")
                    .license_type(gtk::License::Gpl30)
                    .build();
                dialog.present(Some(&window));
            }
        ));
        window.add_action(&about_action);

        window
    }

    fn build_update_page() -> UpdatePageResult {
        let page_box = gtk::Box::new(gtk::Orientation::Vertical, 0);

        let scrolled = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();

        let clamp = adw::Clamp::builder()
            .maximum_size(800)
            .tightening_threshold(600)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .build();

        let content_box = gtk::Box::new(gtk::Orientation::Vertical, 18);

        // ── Hero area ────────────────────────────────────────────────
        let hero_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(14)
            .css_classes(vec!["up-hero"])
            .build();

        let hero_icon = gtk::Image::builder()
            .icon_name("io.github.up")
            .pixel_size(52)
            .build();

        let hero_text_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        hero_text_box.set_valign(gtk::Align::Center);

        let hero_title = gtk::Label::builder()
            .label("System Updater")
            .halign(gtk::Align::Start)
            .css_classes(vec!["up-hero-title"])
            .build();

        let status_label = gtk::Label::builder()
            .label("Detecting available updates across your system…")
            .halign(gtk::Align::Start)
            .css_classes(vec!["up-hero-subtitle"])
            .wrap(true)
            .build();

        hero_text_box.append(&hero_title);
        hero_text_box.append(&status_label);

        let hero_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        hero_spacer.set_hexpand(true);

        let hero_button_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        hero_button_box.set_valign(gtk::Align::Center);

        hero_box.append(&hero_icon);
        hero_box.append(&hero_text_box);
        hero_box.append(&hero_spacer);
        hero_box.append(&hero_button_box);
        content_box.append(&hero_box);

        let progress_bar = gtk::ProgressBar::new();
        progress_bar.set_fraction(0.0);
        progress_bar.set_show_text(false);
        progress_bar.set_margin_top(4);
        progress_bar.set_margin_bottom(4);
        progress_bar.set_margin_start(0);
        progress_bar.set_margin_end(0);
        progress_bar.set_visible(false);
        content_box.append(&progress_bar);

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
            .css_classes(vec!["vex-sources-group"])
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

        // Log panel (expandable terminal output) — appended to page_box below the
        // scroll area so it can fill remaining vertical space when expanded.
        let log_panel = LogPanel::new();

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
            .valign(gtk::Align::Center)
            .sensitive(false)
            .build();

        let cancel_button = gtk::Button::builder()
            .label("Cancel")
            .css_classes(vec!["pill", "up-cancel"])
            .valign(gtk::Align::Center)
            .visible(false)
            .build();

        let updating: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let total_backends: Rc<Cell<usize>> = Rc::new(Cell::new(0));
        let finished_backends: Rc<Cell<usize>> = Rc::new(Cell::new(0));
        let bypass_metered: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let bypass_battery: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let cancel_handle: Rc<RefCell<Option<crate::orchestrator::CancelHandle>>> =
            Rc::new(RefCell::new(None));

        cancel_button.connect_clicked(glib::clone!(
            #[strong]
            cancel_handle,
            move |btn| {
                if let Some(handle) = cancel_handle.borrow_mut().take() {
                    handle.cancel();
                }
                btn.set_sensitive(false);
            }
        ));

        hero_button_box.append(&cancel_button);
        hero_button_box.append(&update_button);

        update_button.connect_clicked(glib::clone!(
            #[weak]
            status_label,
            #[weak]
            progress_bar,
            #[strong]
            rows,
            #[strong]
            log_panel,
            #[strong]
            detected,
            #[weak]
            restart_banner,
            #[strong]
            updating,
            #[strong]
            total_backends,
            #[strong]
            finished_backends,
            #[strong]
            bypass_metered,
            #[strong]
            bypass_battery,
            #[weak]
            cancel_button,
            #[strong]
            cancel_handle,
            move |button| {
                let monitor = gio::NetworkMonitor::default();
                if monitor.is_network_metered() && !bypass_metered.get() {
                    let dialog = adw::AlertDialog::new(
                        Some("Metered Connection"),
                        Some("You are on a metered connection. Downloading updates may use significant data.\n\nContinue anyway?"),
                    );
                    dialog.add_response("cancel", "Cancel");
                    dialog.add_response("update", "Update Anyway");
                    dialog.set_default_response(Some("cancel"));
                    dialog.set_close_response("cancel");
                    dialog.connect_response(None, glib::clone!(
                        #[weak]
                        button,
                        #[strong]
                        bypass_metered,
                        move |_, response| {
                            if response == "update" {
                                bypass_metered.set(true);
                                button.emit_clicked();
                                bypass_metered.set(false);
                            }
                        }
                    ));
                    dialog.present(Some(button));
                    return;
                }
                // Battery check
                if !bypass_battery.get() {
                    if let Some(bat) = crate::battery::read_battery() {
                        if bat.discharging && bat.capacity < 40 {
                            let msg = format!(
                                "Battery is at {}% and discharging. Updates may be interrupted if the device shuts down. Continue anyway?",
                                bat.capacity
                            );
                            let dialog = adw::AlertDialog::new(Some("Low Battery"), Some(&msg));
                            dialog.add_response("cancel", "Cancel");
                            dialog.add_response("update", "Update Anyway");
                            dialog.set_default_response(Some("cancel"));
                            dialog.set_close_response("cancel");
                            dialog.connect_response(
                                None,
                                glib::clone!(
                                    #[weak]
                                    button,
                                    #[strong]
                                    bypass_battery,
                                    move |_, response| {
                                        if response == "update" {
                                            bypass_battery.set(true);
                                            button.emit_clicked();
                                            bypass_battery.set(false);
                                        }
                                    }
                                ),
                            );
                            dialog.present(Some(button));
                            return;
                        }
                    }
                }
                button.set_sensitive(false);
                updating.set(true);
                log_panel.clear();
                cancel_button.set_visible(true);

                // Visually mark skipped rows before starting; collect only active backends.
                {
                    let borrowed = rows.borrow();
                    for (_, row) in borrowed.iter() {
                        if row.is_skipped() {
                            row.set_status_skipped("Skipped by user");
                        }
                    }
                }
                let backends: Vec<_> = {
                    let detected_borrow = detected.borrow();
                    let rows_borrow = rows.borrow();
                    detected_borrow
                        .iter()
                        .filter(|b| {
                            rows_borrow
                                .iter()
                                .find(|(k, _)| *k == b.kind())
                                .map(|(_, r)| !r.is_skipped())
                                .unwrap_or(true)
                        })
                        .cloned()
                        .map(|b| (b, None))
                        .collect()
                };

                let n_backends = backends.len();
                total_backends.set(n_backends);
                finished_backends.set(0);
                progress_bar.set_fraction(0.0);
                progress_bar.set_visible(true);

                glib::spawn_future_local(glib::clone!(
                    #[strong]
                    rows,
                    #[strong]
                    log_panel,
                    #[weak]
                    status_label,
                    #[weak]
                    progress_bar,
                    #[weak]
                    button,
                    #[weak]
                    restart_banner,
                    #[strong]
                    updating,
                    #[strong]
                    total_backends,
                    #[strong]
                    finished_backends,
                    #[weak]
                    cancel_button,
                    #[strong]
                    cancel_handle,
                    async move {
                        use crate::orchestrator::{OrchestratorEvent, UpdateOrchestrator};

                        let orchestrator = UpdateOrchestrator::new(backends);
                        let (event_tx, event_rx) = async_channel::unbounded::<OrchestratorEvent>();
                        let handle = orchestrator.run_all(event_tx);
                        *cancel_handle.borrow_mut() = Some(handle);

                        let mut auth_started = false;
                        let mut has_error = false;
                        let mut self_updated = false;
                        let mut nix_log_lines: Vec<String> = Vec::new();

                        while let Ok(event) = event_rx.recv().await {
                            match event {
                                OrchestratorEvent::AuthStarted => {
                                    auth_started = true;
                                    status_label.set_label("Authenticating\u{2026}");
                                    log_panel
                                        .append_line("Requesting administrator privileges\u{2026}");
                                }
                                OrchestratorEvent::AuthSucceeded => {
                                    if auth_started {
                                        log_panel.append_line("Authentication successful.");
                                    }
                                    status_label.set_label("Updating\u{2026}");
                                }
                                OrchestratorEvent::AuthFailed(e) => {
                                    log_panel.append_line(&format!("Authentication failed: {e}"));
                                    status_label.set_label("Update cancelled.");
                                    progress_bar.set_visible(false);
                                    *cancel_handle.borrow_mut() = None;
                                    cancel_button.set_visible(false);
                                    cancel_button.set_sensitive(true);
                                    button.set_sensitive(true);
                                    return;
                                }
                                OrchestratorEvent::BackendStarted(kind) => {
                                    let rows_borrowed = rows.borrow();
                                    if let Some((_, row)) =
                                        rows_borrowed.iter().find(|(k, _)| *k == kind)
                                    {
                                        row.set_status_running();
                                    }
                                    let finished = finished_backends.get();
                                    let total = total_backends.get();
                                    if total > 0 {
                                        progress_bar.set_fraction(
                                            (finished as f64 + 0.5) / total as f64,
                                        );
                                    }
                                }
                                OrchestratorEvent::BackendLog(kind, line) => {
                                    if kind == BackendKind::Nix {
                                        nix_log_lines.push(line.clone());
                                    }
                                    log_panel.append_line(&format!("[{kind}] {line}"));
                                }
                                OrchestratorEvent::BackendFinished(kind, result) => {
                                    let mut show_cache_dialog = false;
                                    {
                                        let rows_borrowed = rows.borrow();
                                        if let Some((_, row)) =
                                            rows_borrowed.iter().find(|(k, _)| *k == kind)
                                        {
                                            match &result {
                                                UpdateResult::Success { updated_count } => {
                                                    row.set_status_success(*updated_count);
                                                }
                                                UpdateResult::SuccessWithSelfUpdate {
                                                    updated_count,
                                                } => {
                                                    row.set_status_success(*updated_count);
                                                    self_updated = true;
                                                }
                                                UpdateResult::Error(msg) => {
                                                    row.set_status_error(&msg.to_string());
                                                    has_error = true;
                                                }
                                                UpdateResult::Skipped(msg) => {
                                                    row.set_status_skipped(msg);
                                                }
                                                UpdateResult::Cancelled => {
                                                    row.set_status_skipped("Cancelled");
                                                }
                                                UpdateResult::CacheMiss => {
                                                    row.set_status_skipped(
                                                        "Binary cache syncing, try again later",
                                                    );
                                                    show_cache_dialog = true;
                                                }
                                            }
                                        }
                                    }
                                    if show_cache_dialog {
                                        let details =
                                            crate::backends::nix::extract_cache_block_message(
                                                &nix_log_lines,
                                            )
                                            .unwrap_or_else(|| {
                                                "No further detail was provided.".to_string()
                                            });
                                        crate::ui::cache_block_dialog::show_cache_block_dialog(
                                            &button,
                                            &details,
                                            glib::clone!(
                                                #[strong]
                                                rows,
                                                #[strong]
                                                log_panel,
                                                #[weak]
                                                status_label,
                                                #[weak]
                                                button,
                                                move || spawn_cache_bypass(
                                                    crate::backends::nix::CacheBypassMode::Deploy,
                                                    rows.clone(),
                                                    log_panel.clone(),
                                                    status_label.clone(),
                                                    button.clone(),
                                                )
                                            ),
                                            glib::clone!(
                                                #[strong]
                                                rows,
                                                #[strong]
                                                log_panel,
                                                #[weak]
                                                status_label,
                                                #[weak]
                                                button,
                                                move || spawn_cache_bypass(
                                                    crate::backends::nix::CacheBypassMode::UpdateAll,
                                                    rows.clone(),
                                                    log_panel.clone(),
                                                    status_label.clone(),
                                                    button.clone(),
                                                )
                                            ),
                                        );
                                    }
                                    let finished = finished_backends.get() + 1;
                                    finished_backends.set(finished);
                                    let total = total_backends.get();
                                    let fraction = if total == 0 {
                                        1.0
                                    } else {
                                        finished as f64 / total as f64
                                    };
                                    progress_bar.set_fraction(fraction);
                                }
                                OrchestratorEvent::AllFinished => {
                                    progress_bar.set_fraction(1.0);
                                    break;
                                }
                            }
                        }

                        if self_updated {
                            restart_banner.set_revealed(true);
                        }
                        if has_error {
                            status_label.set_label("Update completed with errors.");
                        } else {
                            status_label.set_label("Update complete.");
                        }
                        progress_bar.set_visible(false);
                        *cancel_handle.borrow_mut() = None;
                        cancel_button.set_visible(false);
                        cancel_button.set_sensitive(true);
                        updating.set(false);
                        button.set_sensitive(true);
                        if !has_error {
                            // Check if reboot is actually required before prompting.
                            // reboot_required() performs fast filesystem/process checks
                            // and is safe to call on the GTK main thread.
                            let reboot_needed = crate::reboot::reboot_required();
                            if reboot_needed {
                                crate::ui::reboot_dialog::show_reboot_dialog(&button);
                            }
                        }
                    }
                ));
            }
        ));

        clamp.set_child(Some(&content_box));
        scrolled.set_child(Some(&clamp));

        let metered_banner = adw::Banner::new("On a metered connection. Consider updating later.");
        metered_banner.set_use_markup(false);
        let monitor = gio::NetworkMonitor::default();
        metered_banner.set_revealed(monitor.is_network_metered());
        monitor.connect_network_metered_notify(glib::clone!(
            #[weak]
            metered_banner,
            move |m| {
                metered_banner.set_revealed(m.is_network_metered());
            }
        ));

        page_box.append(&restart_banner);
        page_box.append(&metered_banner);
        page_box.append(&scrolled);

        // Dock the log panel below the scrolled content.
        log_panel.expander.set_margin_start(12);
        log_panel.expander.set_margin_end(12);
        log_panel.expander.set_margin_bottom(12);
        page_box.append(&log_panel.expander);

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

                for backend in detected.borrow().iter() {
                    let kind = backend.kind();
                    {
                        let borrowed = rows.borrow();
                        if let Some((_, row)) = borrowed.iter().find(|(k, _)| *k == kind) {
                            row.set_status_checking();
                        }
                    }
                    let backend_clone = backend.clone();
                    glib::spawn_future_local(glib::clone!(
                        #[strong]
                        rows,
                        #[strong]
                        pending_checks,
                        #[strong]
                        total_available,
                        #[weak]
                        update_button_checks,
                        #[weak]
                        status_label_checks,
                        #[strong]
                        check_epoch,
                        async move {
                            type CheckPayload =
                                (Result<usize, String>, Result<Vec<String>, String>);
                            let (tx, rx) = async_channel::bounded::<CheckPayload>(1);
                            super::spawn_background_async(move || async move {
                                let count = backend_clone.count_available().await;
                                let list = backend_clone.list_available().await;
                                let _ = tx.send((count, list)).await;
                            });
                            if let Ok((count_result, list_result)) = rx.recv().await {
                                // Discard results from a superseded check cycle.
                                if check_epoch.get() != my_epoch {
                                    return;
                                }
                                let row = {
                                    let borrowed = rows.borrow();
                                    borrowed
                                        .iter()
                                        .find(|(k, _)| *k == kind)
                                        .map(|(_, r)| r.clone())
                                };
                                let Some(row) = row else {
                                    return;
                                };
                                match count_result {
                                    Ok(count) => {
                                        row.set_status_available(count);
                                        *total_available.borrow_mut() += count;
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
                                    let mut p = pending_checks.borrow_mut();
                                    *p -= 1;
                                    *p
                                };
                                if remaining == 0 {
                                    let non_skipped_total: usize = {
                                        let borrowed = rows.borrow();
                                        borrowed
                                            .iter()
                                            .filter(|(_, r)| !r.is_skipped())
                                            .filter_map(|(_, r)| r.last_available_count())
                                            .sum()
                                    };
                                    let any_check_error = {
                                        let borrowed = rows.borrow();
                                        borrowed
                                            .iter()
                                            .filter(|(_, r)| !r.is_skipped())
                                            .any(|(_, r)| r.has_check_error())
                                    };
                                    if non_skipped_total > 0 {
                                        update_button_checks.set_sensitive(true);
                                        status_label_checks.set_label(&format!(
                                            "{non_skipped_total} update{} available",
                                            if non_skipped_total == 1 { "" } else { "s" }
                                        ));
                                    } else if any_check_error {
                                        status_label_checks
                                            .set_label("Could not check all sources.");
                                    } else {
                                        status_label_checks.set_label("Everything is up to date.");
                                    }
                                }
                            }
                        }
                    ));
                }
            })
        };

        // Spawn backend detection off the GTK thread.
        {
            let (detect_tx, detect_rx) = async_channel::unbounded::<Vec<Arc<dyn Backend>>>();

            super::spawn_background_async(move || async move {
                let backends = crate::backends::detect_backends();
                let _ = detect_tx.send(backends).await;
            });

            glib::spawn_future_local(glib::clone!(
                #[strong]
                detected,
                #[strong]
                rows,
                #[weak]
                backends_group,
                #[strong]
                run_checks,
                #[weak]
                update_button,
                #[strong]
                updating,
                #[strong]
                log_panel,
                #[weak]
                status_label,
                async move {
                    if let Ok(new_backends) = detect_rx.recv().await {
                        // Remove placeholder
                        backends_group.remove(&placeholder_row);
                        // Populate rows
                        {
                            let mut rows_mut = rows.borrow_mut();
                            for backend in &new_backends {
                                let kind = backend.kind();
                                let rows_cb = rows.clone();
                                let button_cb = update_button.clone();
                                let updating_cb = updating.clone();
                                // Clones for the retry closure
                                let rows_retry = rows.clone();
                                let log_panel_retry = log_panel.clone();
                                let status_label_retry = status_label.clone();
                                let detected_retry = detected.clone();
                                let updating_retry = updating.clone();
                                let update_button_retry = update_button.clone();
                                let row = UpdateRow::new(
                                    backend.as_ref(),
                                    move || {
                                        if updating_cb.get() {
                                            return;
                                        }
                                        let borrowed = rows_cb.borrow();
                                        let non_skipped_available: usize = borrowed
                                            .iter()
                                            .filter(|(_, r)| !r.is_skipped())
                                            .filter_map(|(_, r)| r.last_available_count())
                                            .sum();
                                        button_cb.set_sensitive(non_skipped_available > 0);
                                    },
                                    move || {
                                        use crate::orchestrator::{
                                            OrchestratorEvent, UpdateOrchestrator,
                                        };
                                        if updating_retry.get() {
                                            return;
                                        }
                                        let backend = {
                                            let detected_borrow = detected_retry.borrow();
                                            detected_borrow
                                                .iter()
                                                .find(|b| b.kind() == kind)
                                                .cloned()
                                        };
                                        let Some(backend) = backend else { return };
                                        updating_retry.set(true);
                                        update_button_retry.set_sensitive(false);
                                        log_panel_retry.append_line(&format!(
                                            "\u{2500}\u{2500}\u{2500} Retrying {kind} \u{2500}\u{2500}\u{2500}"
                                        ));
                                        let orchestrator =
                                            UpdateOrchestrator::new(vec![(backend, None)]);
                                        let (event_tx, event_rx) =
                                            async_channel::unbounded::<OrchestratorEvent>();
                                        orchestrator.run_all(event_tx);
                                        let rows_spawn = rows_retry.clone();
                                        let log_panel_spawn = log_panel_retry.clone();
                                        let updating_spawn = updating_retry.clone();
                                        let update_button_spawn = update_button_retry.clone();
                                        let status_label_spawn = status_label_retry.clone();
                                        glib::spawn_future_local(async move {
                                            let mut nix_log_lines: Vec<String> = Vec::new();
                                            while let Ok(event) = event_rx.recv().await {
                                                match event {
                                                    OrchestratorEvent::AuthFailed(e) => {
                                                        log_panel_spawn.append_line(&format!(
                                                            "Authentication failed: {e}"
                                                        ));
                                                    }
                                                    OrchestratorEvent::BackendStarted(k) => {
                                                        let rows_borrowed = rows_spawn.borrow();
                                                        if let Some((_, row)) = rows_borrowed
                                                            .iter()
                                                            .find(|(rk, _)| *rk == k)
                                                        {
                                                            row.set_status_running();
                                                        }
                                                    }
                                                    OrchestratorEvent::BackendLog(k, line) => {
                                                        if k == BackendKind::Nix {
                                                            nix_log_lines.push(line.clone());
                                                        }
                                                        log_panel_spawn
                                                            .append_line(&format!("[{k}] {line}"));
                                                    }
                                                    OrchestratorEvent::BackendFinished(
                                                        k,
                                                        result,
                                                    ) => {
                                                        let mut show_cache_dialog = false;
                                                        {
                                                            let rows_borrowed = rows_spawn.borrow();
                                                            if let Some((_, row)) = rows_borrowed
                                                                .iter()
                                                                .find(|(rk, _)| *rk == k)
                                                            {
                                                                match &result {
                                                                    UpdateResult::Success {
                                                                        updated_count,
                                                                    } => {
                                                                        row.set_status_success(
                                                                            *updated_count,
                                                                        );
                                                                    }
                                                                    UpdateResult::SuccessWithSelfUpdate {
                                                                        updated_count,
                                                                    } => {
                                                                        row.set_status_success(
                                                                            *updated_count,
                                                                        );
                                                                    }
                                                                    UpdateResult::Error(msg) => {
                                                                        row.set_status_error(
                                                                            &msg.to_string(),
                                                                        );
                                                                    }
                                                                    UpdateResult::Skipped(msg) => {
                                                                        row.set_status_skipped(msg);
                                                                    }
                                                                    UpdateResult::Cancelled => {
                                                                        row.set_status_skipped(
                                                                            "Cancelled",
                                                                        );
                                                                    }
                                                                    UpdateResult::CacheMiss => {
                                                                        row.set_status_skipped(
                                                                            "Binary cache syncing, try again later",
                                                                        );
                                                                        show_cache_dialog = true;
                                                                    }
                                                                }
                                                            }
                                                        }
                                                        if show_cache_dialog {
                                                            let details = crate::backends::nix::extract_cache_block_message(&nix_log_lines)
                                                                .unwrap_or_else(|| "No further detail was provided.".to_string());
                                                            crate::ui::cache_block_dialog::show_cache_block_dialog(
                                                                &update_button_spawn,
                                                                &details,
                                                                glib::clone!(
                                                                    #[strong] rows_spawn,
                                                                    #[strong] log_panel_spawn,
                                                                    #[strong] status_label_spawn,
                                                                    #[strong] update_button_spawn,
                                                                    move || spawn_cache_bypass(
                                                                        crate::backends::nix::CacheBypassMode::Deploy,
                                                                        rows_spawn.clone(),
                                                                        log_panel_spawn.clone(),
                                                                        status_label_spawn.clone(),
                                                                        update_button_spawn.clone(),
                                                                    )
                                                                ),
                                                                glib::clone!(
                                                                    #[strong] rows_spawn,
                                                                    #[strong] log_panel_spawn,
                                                                    #[strong] status_label_spawn,
                                                                    #[strong] update_button_spawn,
                                                                    move || spawn_cache_bypass(
                                                                        crate::backends::nix::CacheBypassMode::UpdateAll,
                                                                        rows_spawn.clone(),
                                                                        log_panel_spawn.clone(),
                                                                        status_label_spawn.clone(),
                                                                        update_button_spawn.clone(),
                                                                    )
                                                                ),
                                                            );
                                                        }
                                                    }
                                                    OrchestratorEvent::AllFinished => {
                                                        break;
                                                    }
                                                    _ => {}
                                                }
                                            }
                                            updating_spawn.set(false);
                                            let non_skipped_total: usize = {
                                                let borrowed = rows_spawn.borrow();
                                                borrowed
                                                    .iter()
                                                    .filter(|(_, r)| !r.is_skipped())
                                                    .filter_map(|(_, r)| r.last_available_count())
                                                    .sum()
                                            };
                                            if non_skipped_total > 0 {
                                                update_button_spawn.set_sensitive(true);
                                            }
                                            let reboot_needed = crate::reboot::reboot_required();
                                            if reboot_needed {
                                                crate::ui::reboot_dialog::show_reboot_dialog(
                                                    &update_button_spawn,
                                                );
                                            }
                                        });
                                    },
                                );
                                backends_group.add(&row.row);
                                rows_mut.push((backend.kind(), row));
                            }
                        }
                        // Store backends
                        *detected.borrow_mut() = new_backends;
                        // Trigger availability check (enables Update All only if updates are found)
                        (*run_checks)();
                    } else {
                        eprintln!("Backend detection failed; no backends detected.");
                        backends_group.remove(&placeholder_row);
                    }
                }
            ));
        }

        (page_box, run_checks, distro_row, version_row, updating)
    }
}

/// Runs a VexOS cache-bypass command (`just deploy` / `just update-all`)
/// chosen from the cache-block dialog, reporting progress on the existing
/// Nix row and log panel. `button` is disabled while the bypass command
/// runs and re-enabled once it finishes.
fn spawn_cache_bypass(
    mode: crate::backends::nix::CacheBypassMode,
    rows: Rc<RefCell<Vec<(BackendKind, UpdateRow)>>>,
    log_panel: LogPanel,
    status_label: gtk::Label,
    button: gtk::Button,
) {
    use crate::orchestrator::{run_cache_bypass, OrchestratorEvent};

    button.set_sensitive(false);
    let (event_tx, event_rx) = async_channel::unbounded::<OrchestratorEvent>();
    run_cache_bypass(mode, event_tx);

    glib::spawn_future_local(async move {
        while let Ok(event) = event_rx.recv().await {
            match event {
                OrchestratorEvent::AuthStarted => {
                    log_panel.append_line("Requesting administrator privileges\u{2026}");
                }
                OrchestratorEvent::AuthSucceeded => {
                    status_label.set_label("Updating\u{2026}");
                }
                OrchestratorEvent::AuthFailed(e) => {
                    log_panel.append_line(&format!("Authentication failed: {e}"));
                    button.set_sensitive(true);
                    return;
                }
                OrchestratorEvent::BackendStarted(kind) => {
                    let rows_borrowed = rows.borrow();
                    if let Some((_, row)) = rows_borrowed.iter().find(|(k, _)| *k == kind) {
                        row.set_status_running();
                    }
                }
                OrchestratorEvent::BackendLog(kind, line) => {
                    log_panel.append_line(&format!("[{kind}] {line}"));
                }
                OrchestratorEvent::BackendFinished(kind, result) => {
                    let rows_borrowed = rows.borrow();
                    if let Some((_, row)) = rows_borrowed.iter().find(|(k, _)| *k == kind) {
                        match &result {
                            UpdateResult::Success { updated_count } => {
                                row.set_status_success(*updated_count);
                                status_label.set_label("Update complete.");
                            }
                            UpdateResult::SuccessWithSelfUpdate { updated_count } => {
                                row.set_status_success(*updated_count);
                                status_label.set_label("Update complete.");
                            }
                            UpdateResult::Error(msg) => {
                                row.set_status_error(&msg.to_string());
                                status_label.set_label("Update failed.");
                            }
                            UpdateResult::Skipped(msg) => {
                                row.set_status_skipped(msg);
                            }
                            UpdateResult::Cancelled => {
                                row.set_status_skipped("Cancelled");
                            }
                            UpdateResult::CacheMiss => {
                                row.set_status_skipped("Binary cache syncing, try again later");
                            }
                        }
                    }
                }
                OrchestratorEvent::AllFinished => break,
            }
        }
        button.set_sensitive(true);
    });
}
