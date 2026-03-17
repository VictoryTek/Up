use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

use crate::ui::log_panel::LogPanel;
use crate::upgrade;

pub struct UpgradePage;

impl UpgradePage {
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

        // Header
        let header_label = gtk::Label::builder()
            .label("Upgrade your distribution to the next major version.")
            .css_classes(vec!["dim-label"])
            .wrap(true)
            .build();
        content_box.append(&header_label);

        // Distro info group
        let info_group = adw::PreferencesGroup::builder()
            .title("System Information")
            .build();

        let distro_info = upgrade::detect_distro();

        let distro_row = adw::ActionRow::builder()
            .title("Distribution")
            .subtitle(&distro_info.name)
            .build();
        distro_row.add_prefix(&gtk::Image::from_icon_name("computer-symbolic"));
        info_group.add(&distro_row);

        let version_row = adw::ActionRow::builder()
            .title("Current Version")
            .subtitle(&distro_info.version)
            .build();
        info_group.add(&version_row);

        let upgrade_available_row = adw::ActionRow::builder()
            .title("Upgrade Available")
            .subtitle(if distro_info.upgrade_supported {
                "Checking..."
            } else {
                "Not supported for this distribution yet"
            })
            .build();
        info_group.add(&upgrade_available_row);

        // Spawn async task to check upgrade availability
        if distro_info.upgrade_supported {
            let upgrade_row_clone = upgrade_available_row.clone();
            let distro_check = distro_info.clone();
            glib::spawn_future_local(async move {
                let (tx, rx) = async_channel::unbounded::<String>();

                std::thread::spawn(move || {
                    let result = upgrade::check_upgrade_available(&distro_check);
                    let _ = tx.send_blocking(result);
                    drop(tx);
                });

                if let Ok(result_msg) = rx.recv().await {
                    upgrade_row_clone.set_subtitle(&result_msg);
                } else {
                    upgrade_row_clone.set_subtitle("Could not determine upgrade availability");
                }
            });
        }

        if distro_info.id == "nixos" {
            let config_type = upgrade::detect_nixos_config_type();
            let config_label: String = match config_type {
                upgrade::NixOsConfigType::Flake => {
                    let hostname = upgrade::detect_hostname();
                    format!("Flake-based (/etc/nixos#{})", hostname)
                }
                upgrade::NixOsConfigType::LegacyChannel => {
                    "Channel-based (/etc/nixos/configuration.nix)".to_string()
                }
            };
            let config_row = adw::ActionRow::builder()
                .title("NixOS Config Type")
                .subtitle(&config_label)
                .build();
            config_row.add_prefix(&gtk::Image::from_icon_name("emblem-system-symbolic"));
            info_group.add(&config_row);
        }

        content_box.append(&info_group);

        // Prerequisites checklist group
        let prereq_group = adw::PreferencesGroup::builder()
            .title("Prerequisites")
            .description("These checks must pass before upgrading")
            .build();

        let checks: Vec<(&str, &str)> = if distro_info.id == "nixos" {
            vec![
                ("nixos-rebuild available", "system-software-update-symbolic"),
                ("Sufficient disk space (10 GB+)", "drive-harddisk-symbolic"),
                ("Backup recommended", "document-save-symbolic"),
            ]
        } else {
            vec![
                ("All packages up to date", "system-software-update-symbolic"),
                ("Sufficient disk space (10 GB+)", "drive-harddisk-symbolic"),
                ("Backup recommended", "document-save-symbolic"),
            ]
        };

        let check_rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::new(RefCell::new(Vec::new()));
        let check_icons: Rc<RefCell<Vec<gtk::Image>>> = Rc::new(RefCell::new(Vec::new()));
        for (label, icon) in &checks {
            let row = adw::ActionRow::builder()
                .title(*label)
                .subtitle("Checking...")
                .build();
            row.add_prefix(&gtk::Image::from_icon_name(*icon));

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
            .build();

        let upgrade_button = gtk::Button::builder()
            .label("Start Upgrade")
            .css_classes(vec!["destructive-action", "pill"])
            .sensitive(false)
            .build();

        button_box.append(&check_button);
        button_box.append(&upgrade_button);
        content_box.append(&button_box);

        // Backup confirmation
        let backup_check = gtk::CheckButton::builder()
            .label("I have backed up my important data")
            .halign(gtk::Align::Center)
            .build();
        content_box.append(&backup_check);

        // Wire up check button
        let check_rows_clone = check_rows.clone();
        let check_icons_clone = check_icons.clone();
        let upgrade_btn_clone = upgrade_button.clone();
        let log_clone = log_panel.clone();
        let backup_clone = backup_check.clone();
        let distro_clone = distro_info.clone();

        check_button.connect_clicked(move |button| {
            button.set_sensitive(false);
            log_clone.clear();

            let check_rows_ref = check_rows_clone.clone();
            let check_icons_ref = check_icons_clone.clone();
            let upgrade_ref = upgrade_btn_clone.clone();
            let log_ref = log_clone.clone();
            let button_ref = button.clone();
            let backup_ref = backup_clone.clone();
            let distro = distro_clone.clone();

            glib::spawn_future_local(async move {
                let (tx, rx) = async_channel::unbounded::<String>();

                let tx_clone = tx.clone();
                let distro_thread = distro.clone();

                std::thread::spawn(move || {
                    let results = upgrade::run_prerequisite_checks(&distro_thread, &tx_clone);
                    // Send results as serialized
                    let json = serde_json::to_string(&results).unwrap_or_default();
                    let _ = tx_clone.send_blocking(format!("__RESULTS__:{json}"));
                    drop(tx_clone);
                });

                drop(tx);

                let mut all_passed = true;
                while let Ok(msg) = rx.recv().await {
                    if let Some(json) = msg.strip_prefix("__RESULTS__:") {
                        if let Ok(results) = serde_json::from_str::<Vec<upgrade::CheckResult>>(json)
                        {
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
                    } else {
                        log_ref.append_line(&msg);
                    }
                }

                if all_passed {
                    // Enable upgrade button only if backup is confirmed
                    let upgrade_ref2 = upgrade_ref.clone();
                    backup_ref.connect_toggled(move |check| {
                        upgrade_ref2.set_sensitive(check.is_active());
                    });
                    if backup_ref.is_active() {
                        upgrade_ref.set_sensitive(true);
                    }
                }

                button_ref.set_sensitive(true);
            });
        });

        // Auto-trigger prerequisite checks for supported distros
        if distro_info.upgrade_supported {
            check_button.emit_clicked();
        }

        // Wire up upgrade button
        let log_clone2 = log_panel.clone();
        let distro_clone2 = distro_info.clone();

        upgrade_button.connect_clicked(move |button| {
            let dialog = adw::AlertDialog::builder()
                .heading("Confirm System Upgrade")
                .body(format!(
                    "This will upgrade {} from version {} to the next major release.\n\n\
                    This operation may take a long time and require a reboot.\n\n\
                    Are you sure you want to continue?",
                    distro_clone2.name, distro_clone2.version
                ))
                .build();

            dialog.add_response("cancel", "Cancel");
            dialog.add_response("upgrade", "Upgrade");
            dialog.set_response_appearance("upgrade", adw::ResponseAppearance::Destructive);
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");

            let log_ref = log_clone2.clone();
            let button_ref = button.clone();
            let distro = distro_clone2.clone();

            dialog.connect_response(None, move |_dialog, response| {
                if response == "upgrade" {
                    button_ref.set_sensitive(false);
                    log_ref.clear();

                    let log_ref2 = log_ref.clone();
                    let distro2 = distro.clone();
                    let button_ref2 = button_ref.clone();

                    glib::spawn_future_local(async move {
                        let (tx, rx) = async_channel::unbounded::<String>();
                        let tx_clone = tx.clone();

                        std::thread::spawn(move || {
                            upgrade::execute_upgrade(&distro2, &tx_clone);
                            drop(tx_clone);
                        });

                        drop(tx);

                        while let Ok(line) = rx.recv().await {
                            log_ref2.append_line(&line);
                        }

                        button_ref2.set_sensitive(true);
                    });
                }
            });

            // Present dialog requires a parent widget
            let widget = button.clone();
            dialog.present(Some(&widget));
        });

        clamp.set_child(Some(&content_box));
        scrolled.set_child(Some(&clamp));
        page_box.append(&scrolled);

        page_box
    }
}
