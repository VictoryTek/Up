use adw::prelude::*;
use gettextrs::gettext;
use gtk::glib;

/// Present a "Reboot Now / Later" dialog attached to `parent`.
/// Only calls `crate::reboot::reboot()` if the user chooses "Reboot Now".
/// Follows the same `adw::AlertDialog` pattern used in `upgrade_page.rs`.
pub fn show_reboot_dialog(parent: &impl gtk::prelude::IsA<gtk::Widget>) {
    let dialog = adw::AlertDialog::builder()
        .heading(gettext("Reboot Required"))
        .body(gettext(
            "A reboot is recommended to complete the update. \
             Would you like to reboot now?",
        ))
        .build();

    dialog.add_response("later", &gettext("Later"));
    dialog.add_response("reboot", &gettext("Reboot Now"));
    dialog.set_response_appearance("reboot", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("later"));
    dialog.set_close_response("later");

    dialog.connect_response(None, move |_dialog, response| {
        if response == "reboot" {
            let (err_tx, err_rx) = async_channel::bounded::<String>(1);

            // Run in a background thread because `reboot()` blocks on .status().
            // On successful reboot systemd kills the process before it can return.
            // On failure, the error is sent back to the GTK main loop via the channel.
            std::thread::spawn(move || {
                if let Err(e) = crate::reboot::reboot() {
                    let _ = err_tx.send_blocking(e);
                }
            });

            glib::spawn_future_local(async move {
                if let Ok(err_msg) = err_rx.recv().await {
                    let error_dialog = adw::AlertDialog::builder()
                        .heading(gettext("Reboot Failed"))
                        .body(format!(
                            gettext(
                                "The system could not be rebooted.\n\n{}\n\n\
                             Please reboot manually using your system settings or terminal."
                            ),
                            err_msg
                        ))
                        .build();
                    error_dialog.add_response("close", &gettext("Close"));
                    error_dialog.set_default_response(Some("close"));
                    error_dialog.set_close_response("close");
                    error_dialog.present(None::<&gtk::Widget>);
                }
            });
        }
    });

    dialog.present(Some(parent));
}
