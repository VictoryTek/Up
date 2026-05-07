use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

use crate::ui::log_panel::LogPanel;
use crate::upgrade;

enum CheckMsg {
    /// A plain log line to display in the terminal output panel.
    Log(String),
    /// Structured results from all prerequisite checks.
    Results(Vec<upgrade::CheckResult>),
}

pub struct UpgradePage;

impl UpgradePage {
    pub fn build() -> (gtk::Box, async_channel::Sender<upgrade::UpgradePageInit>) {
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

        // Header
        let header_label = gtk::Label::builder()
            .label("Upgrade your distribution to the next major version.")
            .css_classes(vec!["dim-label"])
            .wrap(true)
            .build();
        content_box.append(&header_label);

        // Distro info group
        let info_group = adw::PreferencesGroup::builder()
            .title("Upgrade Status")
            .build();

        let distro_info_state: Rc<RefCell<Option<upgrade::DistroInfo>>> =
            Rc::new(RefCell::new(None));

        let nixos_config_type: Rc<RefCell<Option<upgrade::NixOsConfigType>>> =
            Rc::new(RefCell::new(None));

        let upgrade_available_row = adw::ActionRow::builder()
            .title("Upgrade Available")
            .subtitle("Loading\u{2026}")
            .build();
        info_group.add(&upgrade_available_row);

        content_box.append(&info_group);

        // Prerequisites checklist group
        let prereq_group = adw::PreferencesGroup::builder()
            .title("Prerequisites")
            .description("These checks must pass before upgrading")
            .build();

        let checks: Vec<(&str, &str)> = vec![
            ("All packages up to date", "system-software-update-symbolic"),
            ("Sufficient disk space (10 GB+)", "drive-harddisk-symbolic"),
            ("Backup recommended", "document-save-symbolic"),
        ];

        let check_rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::new(RefCell::new(Vec::new()));
        let check_icons: Rc<RefCell<Vec<gtk::Image>>> = Rc::new(RefCell::new(Vec::new()));
        for (label, icon) in &checks {
            let row = adw::ActionRow::builder()
                .title(*label)
                .subtitle("Checking...")
                .build();
            row.add_prefix(&gtk::Image::from_icon_name(icon));

            let status_icon = gtk::Image::from_icon_name("content-loading-symbolic");
            row.add_suffix(&status_icon);

            prereq_group.add(&row);
            check_rows.borrow_mut().push(row);
            check_icons.borrow_mut().push(status_icon);
        }

        content_box.append(&prereq_group);

        // Log panel
        let log_panel = LogPanel::new();
        content_box.append(&log_panel.expander);

        // Buttons
        let button_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .halign(gtk::Align::Center)
            .spacing(12)
            .margin_top(12)
            .build();

        let check_button = gtk::Button::builder()
            .label("Run Checks")
            .css_classes(vec!["pill"])
            .sensitive(false)
            .build();

        let upgrade_button = gtk::Button::builder()
            .label("Start Upgrade")
            .css_classes(vec!["destructive-action", "pill"])
            .sensitive(false)
            .build();

        // Tracks whether a distro upgrade is actually available.
        // The Start Upgrade button must not be enabled unless this is true.
        let upgrade_available: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
        // Tracks whether all prerequisite checks have passed.
        let all_checks_passed: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));

        button_box.append(&check_button);
        button_box.append(&upgrade_button);
        content_box.append(&button_box);

        // Backup confirmation
        let backup_check = gtk::CheckButton::builder()
            .label("I have backed up my important data")
            .halign(gtk::Align::Center)
            .build();
        content_box.append(&backup_check);

        // Shared closure to recompute upgrade button sensitivity from the three state variables.
        let recompute_state: Rc<dyn Fn()> = {
            let upgrade_btn = upgrade_button.clone();
            let upgrade_available = upgrade_available.clone();
            let all_checks_passed = all_checks_passed.clone();
            let backup_check = backup_check.clone();
            Rc::new(move || {
                let enabled = backup_check.is_active()
                    && *all_checks_passed.borrow()
                    && *upgrade_available.borrow();
                upgrade_btn.set_sensitive(enabled);
            })
        };

        // Wire the backup checkbox once (unconditional) so it doesn't accumulate signal handlers.
        {
            let recompute_for_toggle = recompute_state.clone();
            backup_check.connect_toggled(move |_| {
                recompute_for_toggle();
            });
        }

        // Wire up check button
        let check_rows_clone = check_rows.clone();
        let check_icons_clone = check_icons.clone();
        let log_clone = log_panel.clone();
        let distro_state_for_check = distro_info_state.clone();
        let all_checks_passed_clone = all_checks_passed.clone();
        let recompute_state_for_check = recompute_state.clone();
        check_button.connect_clicked(move |button| {
            let Some(distro) = distro_state_for_check.borrow().clone() else {
                return;
            };
            button.set_sensitive(false);
            log_clone.clear();

            let check_rows_ref = check_rows_clone.clone();
            let check_icons_ref = check_icons_clone.clone();
            let log_ref = log_clone.clone();
            let button_ref = button.clone();
            let all_checks_passed_ref = all_checks_passed_clone.clone();
            let recompute_ref = recompute_state_for_check.clone();

            glib::spawn_future_local(async move {
                let (check_tx, check_rx) = async_channel::unbounded::<CheckMsg>();

                let check_tx_clone = check_tx.clone();
                let distro_thread = distro.clone();

                std::thread::spawn(move || {
                    let (bridge_tx, bridge_rx) = async_channel::unbounded::<String>();
                    let results = upgrade::run_prerequisite_checks(&distro_thread, &bridge_tx);
                    drop(bridge_tx);
                    while let Ok(line) = bridge_rx.recv_blocking() {
                        let _ = check_tx_clone.send_blocking(CheckMsg::Log(line));
                    }
                    let _ = check_tx_clone.send_blocking(CheckMsg::Results(results));
                    drop(check_tx_clone);
                });

                drop(check_tx);

                let mut all_passed = true;
                while let Ok(msg) = check_rx.recv().await {
                    match msg {
                        CheckMsg::Log(line) => {
                            log_ref.append_line(&line);
                        }
                        CheckMsg::Results(results) => {
                            let rows = check_rows_ref.borrow();
                            let icons = check_icons_ref.borrow();
                            for (i, result) in results.iter().enumerate() {
                                if let Some(row) = rows.get(i) {
                                    row.set_subtitle(&result.message);
                                }
                                if let Some(icon) = icons.get(i) {
                                    if result.passed {
                                        icon.set_icon_name(Some("emblem-ok-symbolic"));
                                    } else {
                                        icon.set_icon_name(Some("dialog-error-symbolic"));
                                        all_passed = false;
                                    }
                                }
                            }
                        }
                    }
                }

                *all_checks_passed_ref.borrow_mut() = all_passed;
                recompute_ref();

                button_ref.set_sensitive(true);
            });
        });

        // Wire up upgrade button
        let log_clone2 = log_panel.clone();
        let distro_state_for_upgrade = distro_info_state.clone();
        let nixos_config_type_for_upgrade = nixos_config_type.clone();

        upgrade_button.connect_clicked(move |button| {
            let Some(distro) = distro_state_for_upgrade.borrow().clone() else {
                return;
            };

            // NixOS Flake: show informational dialog only — do NOT run an upgrade
            if *nixos_config_type_for_upgrade.borrow() == Some(upgrade::NixOsConfigType::Flake) {
                let next_ch = upgrade::next_nixos_channel(&distro.version_id)
                    .unwrap_or_else(|| "the next NixOS release".to_string());
                let next_ver = next_ch.trim_start_matches("nixos-").to_string();
                let current_ver = distro.version_id.clone();

                let dialog = adw::AlertDialog::builder()
                    .heading("Upgrade via Flake")
                    .body(format!(
                        "NixOS {next_ver} may be available, but this system uses Nix Flakes.\n\n\
                         To upgrade, edit /etc/nixos/flake.nix and update your nixpkgs input \
                         to point to the new release:\n\n\
                         \u{2022} Change:  github:NixOS/nixpkgs/nixos-{current_ver}\n\
                         \u{2022} To:      github:NixOS/nixpkgs/nixos-{next_ver}\n\n\
                         Then run:\n\
                         \u{2022} sudo nix flake update /etc/nixos\n\
                         \u{2022} sudo nixos-rebuild switch --flake /etc/nixos"
                    ))
                    .build();
                dialog.add_response("close", "Close");
                dialog.set_default_response(Some("close"));
                dialog.set_close_response("close");
                dialog.present(Some(button));
                return;
            }

            // All other distros (including NixOS LegacyChannel): destructive confirm dialog
            let dialog = adw::AlertDialog::builder()
                .heading("Confirm System Upgrade")
                .body(format!(
                    "This will upgrade {} from version {} to the next major release.\n\n\
                    This operation may take a long time and require a reboot.\n\n\
                    Are you sure you want to continue?",
                    distro.name, distro.version
                ))
                .build();

            dialog.add_response("cancel", "Cancel");
            dialog.add_response("upgrade", "Upgrade");
            dialog.set_response_appearance("upgrade", adw::ResponseAppearance::Destructive);
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");

            let log_ref = log_clone2.clone();
            let button_ref = button.clone();

            dialog.connect_response(None, move |_dialog, response| {
                if response == "upgrade" {
                    button_ref.set_sensitive(false);
                    log_ref.clear();

                    let log_ref2 = log_ref.clone();
                    let distro2 = distro.clone();
                    let button_ref2 = button_ref.clone();
                    let button_ref3 = button_ref.clone();

                    glib::spawn_future_local(async move {
                        let (tx, rx) = async_channel::unbounded::<String>();
                        let tx_clone = tx.clone();
                        let (result_tx, result_rx) =
                            async_channel::bounded::<Result<(), String>>(1);

                        std::thread::spawn(move || {
                            let outcome = upgrade::execute_upgrade(&distro2, &tx_clone);
                            drop(tx_clone);
                            let _ = result_tx.send_blocking(outcome);
                        });

                        drop(tx);

                        while let Ok(line) = rx.recv().await {
                            log_ref2.append_line(&line);
                        }

                        let outcome = result_rx.recv().await.unwrap_or_else(|_| {
                            Err("Upgrade result channel closed unexpectedly".to_string())
                        });
                        button_ref2.set_sensitive(true);

                        match outcome {
                            Ok(()) => {
                                crate::ui::reboot_dialog::show_reboot_dialog(&button_ref3);
                            }
                            Err(e) => {
                                log_ref2.append_line(&format!("Upgrade failed: {e}"));
                            }
                        }
                    });
                }
            });

            // Present dialog requires a parent widget
            let widget = button.clone();
            dialog.present(Some(&widget));
        });

        clamp.set_child(Some(&content_box));
        scrolled.set_child(Some(&clamp));

        // Flake advisory banner — revealed only when NixOS+Flake is detected.
        let flake_banner = adw::Banner::builder()
            .title("Flake-managed system: upgrade via your flake.nix")
            .revealed(false)
            .build();
        page_box.append(&flake_banner);
        page_box.append(&scrolled);

        let (init_tx, init_rx) = async_channel::bounded::<upgrade::UpgradePageInit>(1);

        {
            let nixos_config_type_fill = nixos_config_type.clone();
            let flake_banner_fill = flake_banner.clone();
            let distro_state_fill = distro_info_state.clone();
            let upgrade_available_row_fill = upgrade_available_row.clone();
            let info_group_fill = info_group.clone();
            let check_rows_fill = check_rows.clone();
            let upgrade_available_fill = upgrade_available.clone();
            let check_btn_fill = check_button.clone();
            let recompute_state_for_init = recompute_state.clone();

            glib::spawn_future_local(async move {
                if let Ok(init) = init_rx.recv().await {
                    let info = init.distro;
                    let nixos_extra = init.nixos_extra;

                    upgrade_available_row_fill.set_subtitle(if info.upgrade_supported {
                        "Checking\u{2026}"
                    } else {
                        "Not supported for this distribution yet"
                    });

                    // Conditionally add NixOS config row
                    if let Some((config_type, raw_hostname)) = &nixos_extra {
                        let safe_hostname = glib::markup_escape_text(raw_hostname);
                        let config_label = match config_type {
                            upgrade::NixOsConfigType::Flake => {
                                format!("Flake-based (/etc/nixos#{})", safe_hostname)
                            }
                            upgrade::NixOsConfigType::LegacyChannel => {
                                "Channel-based (/etc/nixos/configuration.nix)".to_string()
                            }
                        };
                        let config_row = adw::ActionRow::builder()
                            .title("NixOS Config Type")
                            .subtitle(&config_label)
                            .build();
                        config_row
                            .add_prefix(&gtk::Image::from_icon_name("emblem-system-symbolic"));
                        info_group_fill.add(&config_row);
                        // Store config type and reveal banner for flake systems
                        *nixos_config_type_fill.borrow_mut() = Some(config_type.clone());
                        if *config_type == upgrade::NixOsConfigType::Flake {
                            flake_banner_fill.set_revealed(true);
                        }
                        // Update first check row title for NixOS
                        if let Some(row) = check_rows_fill.borrow().first() {
                            row.set_title("nixos-rebuild available");
                        }
                    }

                    // Store distro info
                    *distro_state_fill.borrow_mut() = Some(info.clone());

                    // Spawn upgrade availability check if supported
                    if info.upgrade_supported {
                        let upgrade_row_clone = upgrade_available_row_fill.clone();
                        let distro_check = info.clone();
                        let upgrade_available_clone = upgrade_available_fill.clone();
                        let recompute_for_avail = recompute_state_for_init.clone();
                        glib::spawn_future_local(async move {
                            let (tx, rx) = async_channel::unbounded::<String>();
                            std::thread::spawn(move || {
                                let result = upgrade::check_upgrade_available(&distro_check);
                                let _ = tx.send_blocking(result);
                                drop(tx);
                            });
                            if let Ok(result_msg) = rx.recv().await {
                                let is_available = result_msg.starts_with("Yes");
                                *upgrade_available_clone.borrow_mut() = is_available;
                                upgrade_row_clone.set_subtitle(&result_msg);
                                recompute_for_avail();
                            } else {
                                upgrade_row_clone
                                    .set_subtitle("Could not determine upgrade availability");
                            }
                        });
                    }

                    // Enable check button and auto-trigger checks if supported
                    check_btn_fill.set_sensitive(true);
                    if info.upgrade_supported {
                        check_btn_fill.emit_clicked();
                    }
                }
            });
        }

        (page_box, init_tx)
    }
}
