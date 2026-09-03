use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::ui::history_page::HistoryPage;
use crate::ui::log_panel::LogPanel;
use crate::ui::update_row::UpdateRow;
use crate::ui::upgrade_page::UpgradePage;
use crate::upgrade;
use adw::prelude::*;
use gettextrs::{gettext, ngettext};
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
    Rc<dyn Fn()>,
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
        let (
            update_page,
            run_checks,
            sysinfo_distro_row,
            sysinfo_version_row,
            update_in_progress,
            run_cleanup,
        ) = Self::build_update_page();
        view_stack.add_titled_with_icon(
            &update_page,
            Some("update"),
            &gettext("Update"),
            "software-update-available-symbolic",
        );

        // --- Upgrade Page ---
        let (upgrade_widget, upgrade_init_tx) = UpgradePage::build();
        let upgrade_stack_page = view_stack.add_titled_with_icon(
            &upgrade_widget,
            Some("upgrade"),
            &gettext("Upgrade"),
            "software-update-urgent-symbolic",
        );

        // --- History Page ---
        let history_page = HistoryPage::build();
        view_stack.add_titled_with_icon(
            &history_page,
            Some("history"),
            &gettext("History"),
            "document-open-recent-symbolic",
        );

        // ViewSwitcher lives in the header bar center slot.
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

            glib::spawn_future_local(async move {
                if let Ok((info, nixos_extra)) = detect_rx.recv().await {
                    // 1. Populate update-page system info rows
                    sysinfo_distro_row.set_subtitle(&info.name);
                    sysinfo_version_row.set_subtitle(&info.version);

                    // 2. Gate upgrade tab visibility — hide for unsupported distros.
                    if !info.upgrade_supported {
                        upgrade_stack_page.set_visible(false);
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
            .tooltip_text(gettext("Check for updates"))
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
        app_menu.append(Some(&gettext("Clean Up")), Some("win.cleanup"));
        app_menu.append(Some(&gettext("Preferences")), Some("win.preferences"));
        app_menu.append(Some(&gettext("About Up")), Some("win.about"));
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&app_menu)
            .tooltip_text(gettext("Main menu"))
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

        // Register the "cleanup" window action that runs maintenance for
        // every backend that supports it.
        let cleanup_action = gio::SimpleAction::new("cleanup", None);
        cleanup_action.connect_activate(move |_, _| (*run_cleanup)());
        window.add_action(&cleanup_action);

        // Register the "preferences" window action.
        let preferences_action = gio::SimpleAction::new("preferences", None);
        preferences_action.connect_activate(glib::clone!(
            #[weak]
            window,
            #[upgrade_or]
            return,
            move |_, _| {
                crate::ui::preferences_dialog::show_preferences_dialog(&window);
            }
        ));
        window.add_action(&preferences_action);

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
            .label(gettext("System Updater"))
            .halign(gtk::Align::Start)
            .css_classes(vec!["up-hero-title"])
            .build();

        let status_label = gtk::Label::builder()
            .label(gettext("Detecting available updates across your system…"))
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
        // Kept permanently visible so its footprint is reserved in the layout;
        // opacity is toggled instead of visibility so revealing it on update
        // start does not shift the page down and spawn a vertical scroll bar.
        progress_bar.set_opacity(0.0);
        content_box.append(&progress_bar);

        // System Information group (populated after background distro detection)
        let sys_info_group = adw::PreferencesGroup::builder()
            .title(gettext("System Information"))
            .build();

        let distro_row = adw::ActionRow::builder()
            .title(gettext("Distribution"))
            .subtitle("Loading\u{2026}")
            .build();
        distro_row.add_prefix(&gtk::Image::from_icon_name("computer-symbolic"));
        sys_info_group.add(&distro_row);

        let version_row = adw::ActionRow::builder()
            .title(gettext("Current Version"))
            .subtitle("Loading\u{2026}")
            .build();
        sys_info_group.add(&version_row);

        content_box.append(&sys_info_group);

        // Backend rows group
        let backends_group = adw::PreferencesGroup::builder()
            .title(gettext("Sources"))
            .description(gettext("Package managers detected on this system"))
            .css_classes(vec!["vex-sources-group"])
            .build();

        let detected: Rc<RefCell<Vec<Arc<dyn Backend>>>> = Rc::new(RefCell::new(Vec::new()));

        let rows: Rc<RefCell<Vec<(BackendKind, UpdateRow)>>> = Rc::new(RefCell::new(Vec::new()));

        // Placeholder row shown while background detection runs
        let placeholder_row = adw::ActionRow::builder()
            .title(gettext("Detecting package managers\u{2026}"))
            .build();
        let placeholder_spinner = gtk::Spinner::new();
        placeholder_spinner.start();
        placeholder_row.add_suffix(&placeholder_spinner);
        backends_group.add(&placeholder_row);

        content_box.append(&backends_group);

        // Log panel (expandable terminal output). Kept inside the same
        // scrolled content_box as Sources rather than docked below the
        // scroll area — a sibling with its own fixed height competed with
        // the scrolled region for the window's available height, so once
        // the progress bar became visible on update start there wasn't
        // enough room for both and the Sources rows were clipped behind
        // the log panel. Living in the same scrollable column means the
        // whole page scrolls together and nothing gets clipped.
        let log_panel = LogPanel::new();
        log_panel.expander.set_margin_start(12);
        log_panel.expander.set_margin_end(12);
        log_panel.expander.set_margin_bottom(12);
        content_box.append(&log_panel.expander);

        // Restart notification banner, revealed only when Up itself is updated
        // inside the Flatpak sandbox (new deployment is available on next launch).
        let restart_banner = adw::Banner::builder()
            .title(gettext("Up was updated \u{2014} restart to apply changes"))
            .button_label("Close Up")
            .revealed(false)
            .build();
        restart_banner.connect_button_clicked(|banner| {
            if let Some(window) = banner.root().and_then(|r| r.downcast::<gtk::Window>().ok()) {
                window.close();
            }
        });

        // Completion banner, revealed once an Update All run finishes so the
        // result is obvious even if the user looked away. Hidden again when the
        // next check or update starts.
        let done_banner = adw::Banner::builder()
            .title(gettext("System is up to date"))
            .revealed(false)
            .build();
        done_banner.add_css_class("up-done-banner");

        // Update All button
        let update_button = gtk::Button::builder()
            .label(gettext("Update All"))
            .css_classes(vec!["suggested-action", "pill"])
            .valign(gtk::Align::Center)
            .sensitive(false)
            .build();

        let cancel_button = gtk::Button::builder()
            .label(gettext("Cancel"))
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
            #[weak]
            done_banner,
            move |button| {
                let monitor = gio::NetworkMonitor::default();
                if monitor.is_network_metered() && !bypass_metered.get() {
                    let dialog = adw::AlertDialog::new(
                        Some("Metered Connection"),
                        Some("You are on a metered connection. Downloading updates may use significant data.\n\nContinue anyway?"),
                    );
                    dialog.add_response("cancel", &gettext("Cancel"));
                    dialog.add_response("update", &gettext("Update Anyway"));
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
                            dialog.add_response("cancel", &gettext("Cancel"));
                            dialog.add_response("update", &gettext("Update Anyway"));
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
                done_banner.set_revealed(false);
                log_panel.clear();
                cancel_button.set_visible(true);

                // Visually mark skipped rows before starting; collect only active backends.
                {
                    let borrowed = rows.borrow();
                    for (_, row) in borrowed.iter() {
                        if row.is_skipped() {
                            row.set_status_skipped(&gettext("Skipped by user"));
                        }
                    }
                }
                let backends: Vec<_> = {
                    let detected_borrow = detected.borrow();
                    let rows_borrow = rows.borrow();
                    detected_borrow
                        .iter()
                        .filter_map(|b| {
                            let Some((_, row)) =
                                rows_borrow.iter().find(|(k, _)| *k == b.kind())
                            else {
                                return Some((b.clone(), None));
                            };
                            if row.is_skipped() {
                                return None;
                            }
                            match row.selected_items() {
                                Some(items) if items.is_empty() => {
                                    row.set_status_skipped(&gettext("No packages selected"));
                                    None
                                }
                                selected => Some((b.clone(), selected)),
                            }
                        })
                        .collect()
                };

                let n_backends = backends.len();
                total_backends.set(n_backends);
                finished_backends.set(0);
                progress_bar.set_fraction(0.0);
                progress_bar.set_opacity(1.0);

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
                    #[weak]
                    done_banner,
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
                                    status_label.set_label(&gettext("Authenticating\u{2026}"));
                                    log_panel
                                        .append_line(&gettext("Requesting administrator privileges\u{2026}"));
                                }
                                OrchestratorEvent::AuthSucceeded => {
                                    if auth_started {
                                        log_panel.append_line(&gettext("Authentication successful."));
                                    }
                                    status_label.set_label(&gettext("Updating\u{2026}"));
                                }
                                OrchestratorEvent::AuthFailed(e) => {
                                    log_panel.append_line(&gettext("Authentication failed: {}").replace("{}", &e));
                                    status_label.set_label(&gettext("Update cancelled."));
                                    progress_bar.set_opacity(0.0);
                                    *cancel_handle.borrow_mut() = None;
                                    cancel_button.set_visible(false);
                                    cancel_button.set_sensitive(true);
                                    updating.set(false);
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
                                        // Floor of this backend's segment; the bar
                                        // advances from here on BackendProgress.
                                        progress_bar
                                            .set_fraction(finished as f64 / total as f64);
                                    }
                                }
                                OrchestratorEvent::BackendLog(kind, line) => {
                                    if kind == BackendKind::Nix {
                                        nix_log_lines.push(line.clone());
                                    }
                                    log_panel.append_line(&format!("[{kind}] {line}"));
                                }
                                OrchestratorEvent::BackendProgress(fraction) => {
                                    let total = total_backends.get();
                                    if total > 0 {
                                        let finished = finished_backends.get() as f64;
                                        let target = (finished + fraction.clamp(0.0, 1.0))
                                            / total as f64;
                                        // Monotonic: never let a backend pull the bar back.
                                        if target > progress_bar.fraction() {
                                            progress_bar.set_fraction(target);
                                        }
                                    }
                                }
                                OrchestratorEvent::BackendFinished(kind, result) => {
                                    let outcome = apply_backend_finished(
                                        &kind,
                                        &result,
                                        &rows,
                                        &log_panel,
                                        &status_label,
                                        &button,
                                        &nix_log_lines,
                                    );
                                    has_error |= outcome.is_error;
                                    self_updated |= outcome.is_self_update;
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
                            status_label.set_label(&gettext("Update completed with errors."));
                            done_banner.set_title(&gettext(
                                "Update finished with errors \u{2014} see the log below",
                            ));
                        } else {
                            status_label.set_label(&gettext("Update complete."));
                            done_banner.set_title(&gettext("System is up to date"));
                        }
                        done_banner.set_revealed(true);
                        progress_bar.set_opacity(0.0);
                        *cancel_handle.borrow_mut() = None;
                        cancel_button.set_visible(false);
                        cancel_button.set_sensitive(true);
                        updating.set(false);
                        // Re-gate Update All: a backend that updated cleanly now
                        // reports zero available, so the button only stays live
                        // if an un-skipped backend still has outstanding updates
                        // (e.g. one that errored out).
                        let remaining: usize = {
                            let borrowed = rows.borrow();
                            borrowed
                                .iter()
                                .filter(|(_, r)| !r.is_skipped())
                                .filter_map(|(_, r)| r.last_available_count())
                                .sum()
                        };
                        button.set_sensitive(remaining > 0);
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

        // Revealed by the availability check when the estimated update size
        // exceeds the free space on the root filesystem.
        let low_space_banner = adw::Banner::new("");
        low_space_banner.set_use_markup(false);
        low_space_banner.set_revealed(false);

        page_box.append(&restart_banner);
        page_box.append(&done_banner);
        page_box.append(&metered_banner);
        page_box.append(&low_space_banner);
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
            let low_space_banner = low_space_banner.clone();
            let done_banner = done_banner.clone();
            Rc::new(move || {
                let n = detected.borrow().len();
                if n == 0 {
                    return;
                }
                // Disable button and reset counters at the start of each check cycle.
                update_button_checks.set_sensitive(false);
                low_space_banner.set_revealed(false);
                done_banner.set_revealed(false);
                *pending_checks.borrow_mut() = n;
                *total_available.borrow_mut() = 0;
                // Increment epoch to invalidate in-flight futures from the previous check.
                check_epoch.set(check_epoch.get() + 1);
                let my_epoch = check_epoch.get();
                status_label_checks.set_label(&gettext("Checking for updates\u{2026}"));

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
                        #[weak]
                        low_space_banner,
                        async move {
                            type CheckPayload = (
                                Result<usize, String>,
                                Result<Vec<String>, String>,
                                Option<u64>,
                            );
                            let (tx, rx) = async_channel::bounded::<CheckPayload>(1);
                            super::spawn_background_async(move || async move {
                                let executor = crate::executor::SystemExecutor;
                                let count = backend_clone.count_available(&executor).await;
                                let list = backend_clone.list_available(&executor).await;
                                let size = backend_clone.estimate_size(&executor).await;
                                let _ = tx.send((count, list, size)).await;
                            });
                            if let Ok((count_result, list_result, size_result)) = rx.recv().await {
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
                                row.set_estimated_size(size_result);
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
                                    // Sum the per-backend size estimates for non-skipped rows.
                                    // `None` when no backend could estimate anything.
                                    let estimated_size: Option<u64> = {
                                        let borrowed = rows.borrow();
                                        borrowed
                                            .iter()
                                            .filter(|(_, r)| !r.is_skipped())
                                            .filter_map(|(_, r)| r.last_estimated_size())
                                            .reduce(|a, b| a.saturating_add(b))
                                    };
                                    if non_skipped_total > 0 {
                                        update_button_checks.set_sensitive(true);
                                        let size_suffix = match estimated_size {
                                            Some(bytes) if bytes > 0 => gettext(" (~{})")
                                                .replace("{}", &crate::disk::format_bytes(bytes)),
                                            _ => String::new(),
                                        };
                                        let base = ngettext(
                                            "{} update available",
                                            "{} updates available",
                                            non_skipped_total as u32,
                                        )
                                        .replace("{}", &non_skipped_total.to_string());
                                        status_label_checks
                                            .set_label(&format!("{base}{size_suffix}"));
                                        maybe_warn_low_space(&low_space_banner, estimated_size);
                                    } else if any_check_error {
                                        status_label_checks
                                            .set_label(&gettext("Could not check all sources."));
                                    } else {
                                        status_label_checks
                                            .set_label(&gettext("Everything is up to date."));
                                    }
                                }
                            }
                        }
                    ));
                }
            })
        };

        // Build the cleanup-maintenance closure. Shared with the overflow menu's
        // "Clean Up" action.
        let run_cleanup: Rc<dyn Fn()> = {
            let detected = detected.clone();
            let log_panel = log_panel.clone();
            let status_label = status_label.clone();
            let update_button = update_button.clone();
            let updating = updating.clone();
            Rc::new(move || {
                if updating.get() {
                    return;
                }
                let cleanup_backends: Vec<Arc<dyn Backend>> = detected
                    .borrow()
                    .iter()
                    .filter(|b| b.supports_cleanup())
                    .cloned()
                    .collect();
                if cleanup_backends.is_empty() {
                    status_label.set_label(&gettext("No cleanup available for detected backends."));
                    return;
                }
                updating.set(true);
                update_button.set_sensitive(false);
                log_panel.clear();
                status_label.set_label(&gettext("Starting cleanup\u{2026}"));
                spawn_cleanup(
                    cleanup_backends,
                    log_panel.clone(),
                    status_label.clone(),
                    update_button.clone(),
                    updating.clone(),
                );
            })
        };

        // Spawn backend detection off the GTK thread.
        {
            let (detect_tx, detect_rx) = async_channel::unbounded::<Vec<Arc<dyn Backend>>>();

            super::spawn_background_async(move || async move {
                let disabled = crate::config::load_config().disabled_plugins;
                let backends = crate::backends::detect_backends(&disabled);
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
                #[weak]
                restart_banner,
                async move {
                    if let Ok(new_backends) = detect_rx.recv().await {
                        // Remove placeholder
                        backends_group.remove(&placeholder_row);
                        let config = crate::config::load_config();
                        // Populate rows
                        {
                            let mut rows_mut = rows.borrow_mut();
                            for backend in &new_backends {
                                let kind = backend.kind();
                                let initial_skipped = config.skipped_backends.contains(&kind);
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
                                let restart_banner_retry = restart_banner.clone();
                                let row = UpdateRow::new(
                                    backend.as_ref(),
                                    initial_skipped,
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

                                        let mut cfg = crate::config::load_config();
                                        cfg.skipped_backends = borrowed
                                            .iter()
                                            .filter(|(_, r)| r.is_skipped())
                                            .map(|(k, _)| k.clone())
                                            .collect();
                                        let _ = crate::config::save_config(&cfg);
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
                                            "\u{2500}\u{2500}\u{2500} {} \u{2500}\u{2500}\u{2500}",
                                            gettext("Retrying {}").replace("{}", &kind.to_string())
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
                                        let restart_banner_spawn = restart_banner_retry.clone();
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
                                                        let outcome = apply_backend_finished(
                                                            &k,
                                                            &result,
                                                            &rows_spawn,
                                                            &log_panel_spawn,
                                                            &status_label_spawn,
                                                            &update_button_spawn,
                                                            &nix_log_lines,
                                                        );
                                                        if outcome.is_self_update {
                                                            restart_banner_spawn.set_revealed(true);
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

        (
            page_box,
            run_checks,
            distro_row,
            version_row,
            updating,
            run_cleanup,
        )
    }
}

/// Runs the cleanup/maintenance sequence for every backend that supports
/// it, reporting progress through the log panel and status label.
/// `update_button` is disabled and `updating` set for the duration to
/// keep this mutually exclusive with Update All / Refresh / Retry, all of
/// which already gate on `updating`.
fn spawn_cleanup(
    backends: Vec<Arc<dyn Backend>>,
    log_panel: LogPanel,
    status_label: gtk::Label,
    update_button: gtk::Button,
    updating: Rc<Cell<bool>>,
) {
    use crate::orchestrator::{CleanupOrchestrator, OrchestratorEvent};

    let (event_tx, event_rx) = async_channel::unbounded::<OrchestratorEvent>();
    CleanupOrchestrator::new(backends).run_all(event_tx);

    glib::spawn_future_local(async move {
        let mut has_error = false;
        while let Ok(event) = event_rx.recv().await {
            match event {
                OrchestratorEvent::AuthStarted => {
                    log_panel.append_line(&gettext("Requesting administrator privileges\u{2026}"));
                }
                OrchestratorEvent::AuthSucceeded => {
                    status_label.set_label(&gettext("Cleaning up\u{2026}"));
                }
                OrchestratorEvent::AuthFailed(e) => {
                    log_panel.append_line(&gettext("Authentication failed: {}").replace("{}", &e));
                    status_label.set_label(&gettext("Cleanup cancelled."));
                    updating.set(false);
                    update_button.set_sensitive(true);
                    return;
                }
                OrchestratorEvent::BackendStarted(kind) => {
                    log_panel.append_line(&format!(
                        "\u{2500}\u{2500}\u{2500} {} \u{2500}\u{2500}\u{2500}",
                        gettext("Cleaning {}").replace("{}", &kind.to_string())
                    ));
                }
                OrchestratorEvent::BackendLog(kind, line) => {
                    log_panel.append_line(&format!("[{kind}] {line}"));
                }
                OrchestratorEvent::BackendFinished(kind, result) => match &result {
                    UpdateResult::Success { updated_count, .. }
                    | UpdateResult::SuccessWithSelfUpdate { updated_count, .. } => {
                        log_panel.append_line(&format!(
                            "[{kind}] {}",
                            gettext("Cleanup finished ({} removed)")
                                .replace("{}", &updated_count.to_string())
                        ));
                    }
                    UpdateResult::Error(msg) => {
                        log_panel.append_line(&format!(
                            "[{kind}] {}",
                            gettext("Cleanup failed: {}").replace("{}", &msg.to_string())
                        ));
                        has_error = true;
                    }
                    UpdateResult::Skipped(msg) => {
                        log_panel.append_line(&format!(
                            "[{kind}] {}",
                            gettext("Skipped: {}").replace("{}", msg)
                        ));
                    }
                    UpdateResult::Cancelled => {
                        log_panel.append_line(&format!("[{kind}] {}", gettext("Cancelled")));
                    }
                    UpdateResult::CacheMiss => {
                        log_panel.append_line(&format!(
                            "[{kind}] Binary cache syncing, try again later"
                        ));
                    }
                },
                // Cleanup reports through the log panel, not a progress bar.
                OrchestratorEvent::BackendProgress(_) => {}
                OrchestratorEvent::AllFinished => break,
            }
        }
        status_label.set_label(if has_error {
            "Cleanup completed with errors."
        } else {
            "Cleanup complete."
        });
        updating.set(false);
        update_button.set_sensitive(true);
    });
}

/// Flags folded back into a caller's own loop state by [`apply_backend_finished`].
#[derive(Default)]
struct BackendFinishedOutcome {
    is_error: bool,
    is_self_update: bool,
}

/// Apply a `BackendFinished` result to its update row, surface the VexOS
/// cache-block dialog when the update is on hold, and record the outcome to
/// the history log.
///
/// Shared by the "Update All" event loop and the per-row retry loop so that a
/// new [`UpdateResult`] variant only needs handling in one place. The caller
/// folds the returned flags into its own state (progress bar, restart banner,
/// final status label).
fn apply_backend_finished(
    kind: &BackendKind,
    result: &UpdateResult,
    rows: &Rc<RefCell<Vec<(BackendKind, UpdateRow)>>>,
    log_panel: &LogPanel,
    status_label: &gtk::Label,
    dialog_anchor: &gtk::Button,
    nix_log_lines: &[String],
) -> BackendFinishedOutcome {
    let mut outcome = BackendFinishedOutcome::default();
    let mut show_cache_dialog = false;
    {
        let rows_borrowed = rows.borrow();
        if let Some((_, row)) = rows_borrowed.iter().find(|(k, _)| k == kind) {
            match result {
                UpdateResult::Success {
                    updated_count,
                    updated_items,
                } => {
                    row.set_packages(updated_items);
                    row.set_status_success(*updated_count);
                }
                UpdateResult::SuccessWithSelfUpdate {
                    updated_count,
                    updated_items,
                } => {
                    row.set_packages(updated_items);
                    row.set_status_success(*updated_count);
                    outcome.is_self_update = true;
                }
                UpdateResult::Error(msg) => {
                    row.set_status_error(&msg.to_string());
                    outcome.is_error = true;
                }
                UpdateResult::Skipped(msg) => row.set_status_skipped(msg),
                UpdateResult::Cancelled => row.set_status_skipped(&gettext("Cancelled")),
                UpdateResult::CacheMiss => {
                    row.set_status_skipped(&gettext("Binary cache syncing, try again later"));
                    show_cache_dialog = true;
                }
            }
        }
    }
    if show_cache_dialog {
        let details = crate::backends::nix::extract_cache_block_message(nix_log_lines)
            .unwrap_or_else(|| "No further detail was provided.".to_string());
        crate::ui::cache_block_dialog::show_cache_block_dialog(
            dialog_anchor,
            &details,
            glib::clone!(
                #[strong]
                rows,
                #[strong]
                log_panel,
                #[strong]
                status_label,
                #[strong]
                dialog_anchor,
                move || spawn_cache_bypass(
                    crate::backends::nix::CacheBypassMode::Deploy,
                    rows.clone(),
                    log_panel.clone(),
                    status_label.clone(),
                    dialog_anchor.clone(),
                )
            ),
            glib::clone!(
                #[strong]
                rows,
                #[strong]
                log_panel,
                #[strong]
                status_label,
                #[strong]
                dialog_anchor,
                move || spawn_cache_bypass(
                    crate::backends::nix::CacheBypassMode::UpdateAll,
                    rows.clone(),
                    log_panel.clone(),
                    status_label.clone(),
                    dialog_anchor.clone(),
                )
            ),
        );
    }
    record_history_entry(kind, result);
    outcome
}

/// Reveal `banner` with a warning when the root filesystem has less free space
/// than the pending updates are estimated to need. `needed` is the summed
/// per-backend estimate (bytes); `None`/`0` is a no-op. The `df` probe runs off
/// the GTK thread.
fn maybe_warn_low_space(banner: &adw::Banner, needed: Option<u64>) {
    let Some(needed) = needed.filter(|n| *n > 0) else {
        return;
    };
    let banner = banner.clone();
    let (tx, rx) = async_channel::bounded::<u64>(1);
    super::spawn_background_async(move || async move {
        let _ = tx.send(crate::disk::detect_available_space()).await;
    });
    glib::spawn_future_local(async move {
        if let Ok(available) = rx.recv().await {
            if available > 0 && available < needed {
                banner.set_title(&format!(
                    "Low disk space: {} free, updates need about {}",
                    crate::disk::format_bytes(available),
                    crate::disk::format_bytes(needed),
                ));
                banner.set_revealed(true);
            }
        }
    });
}

/// Records a finished backend's outcome to the persistent update history log.
///
/// Best-effort: write failures are discarded, matching the existing
/// convention for non-critical history I/O (see `history_page.rs`'s
/// clear-history handler).
fn record_history_entry(kind: &BackendKind, result: &UpdateResult) {
    let (result_str, updated_count, error) = match result {
        UpdateResult::Success { updated_count, .. } => ("success", Some(*updated_count), None),
        UpdateResult::SuccessWithSelfUpdate { updated_count, .. } => {
            ("success_self_update", Some(*updated_count), None)
        }
        UpdateResult::Error(msg) => ("error", None, Some(msg.to_string())),
        UpdateResult::Skipped(_) | UpdateResult::Cancelled | UpdateResult::CacheMiss => {
            ("skipped", None, None)
        }
    };
    let entry = crate::history::HistoryEntry {
        timestamp: crate::history::now_secs(),
        backend: kind.to_string(),
        result: result_str.to_string(),
        updated_count,
        error,
    };
    let _ = crate::history::append_entry(&entry);
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
                    log_panel.append_line(&gettext("Requesting administrator privileges\u{2026}"));
                }
                OrchestratorEvent::AuthSucceeded => {
                    status_label.set_label(&gettext("Updating\u{2026}"));
                }
                OrchestratorEvent::AuthFailed(e) => {
                    log_panel.append_line(&gettext("Authentication failed: {}").replace("{}", &e));
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
                            UpdateResult::Success {
                                updated_count,
                                updated_items,
                            } => {
                                row.set_packages(updated_items);
                                row.set_status_success(*updated_count);
                                status_label.set_label(&gettext("Update complete."));
                            }
                            UpdateResult::SuccessWithSelfUpdate {
                                updated_count,
                                updated_items,
                            } => {
                                row.set_packages(updated_items);
                                row.set_status_success(*updated_count);
                                status_label.set_label(&gettext("Update complete."));
                            }
                            UpdateResult::Error(msg) => {
                                row.set_status_error(&msg.to_string());
                                status_label.set_label(&gettext("Update failed."));
                            }
                            UpdateResult::Skipped(msg) => {
                                row.set_status_skipped(msg);
                            }
                            UpdateResult::Cancelled => {
                                row.set_status_skipped(&gettext("Cancelled"));
                            }
                            UpdateResult::CacheMiss => {
                                row.set_status_skipped(&gettext(
                                    "Binary cache syncing, try again later",
                                ));
                            }
                        }
                    }
                }
                // The cache-bypass flow has no progress bar of its own.
                OrchestratorEvent::BackendProgress(_) => {}
                OrchestratorEvent::AllFinished => break,
            }
        }
        button.set_sensitive(true);
    });
}
