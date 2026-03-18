use adw::prelude::*;

/// Present a "Reboot Now / Later" dialog attached to `parent`.
/// Only calls `crate::reboot::reboot()` if the user chooses "Reboot Now".
/// Follows the same `adw::AlertDialog` pattern used in `upgrade_page.rs`.
pub fn show_reboot_dialog(parent: &impl gtk::prelude::IsA<gtk::Widget>) {
    let dialog = adw::AlertDialog::builder()
        .heading("Reboot Required")
        .body(
            "A reboot is recommended to complete the update. \
             Would you like to reboot now?",
        )
        .build();

    dialog.add_response("later", "Later");
    dialog.add_response("reboot", "Reboot Now");
    dialog.set_response_appearance("reboot", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("later"));
    dialog.set_close_response("later");

    dialog.connect_response(None, move |_dialog, response| {
        if response == "reboot" {
            crate::reboot::reboot();
        }
    });

    dialog.present(Some(parent));
}
