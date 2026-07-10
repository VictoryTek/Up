use adw::prelude::*;
use gettextrs::gettext;

/// Present the VexOS cache-block dialog. `details` is the extracted
/// VEXOS_CACHE_BLOCK explanation (already newline-joined, prefix-stripped).
/// `on_deploy` / `on_update_all` are invoked when the user picks the
/// respective bypass option. Choosing "Wait" closes the window containing
/// `parent`, which quits Up since it holds no other `Application` reference.
pub fn show_cache_block_dialog(
    parent: &(impl gtk::prelude::IsA<gtk::Widget> + Clone),
    details: &str,
    on_deploy: impl Fn() + 'static,
    on_update_all: impl Fn() + 'static,
) {
    let body = format!(
        "{}\n\n{}",
        gettext(
            "VexOS paused this update because some packages require a local \
             source build that Hydra's binary cache has not finished yet."
        ),
        details
    );

    let dialog = adw::AlertDialog::builder()
        .heading(gettext("Update Blocked — Binary Cache Catching Up"))
        .body(body)
        .build();

    dialog.add_response("wait", &gettext("Wait"));
    dialog.add_response("deploy", &gettext("Just Deploy"));
    dialog.add_response("update-all", &gettext("Update All Now"));
    dialog.set_response_appearance("deploy", adw::ResponseAppearance::Suggested);
    dialog.set_response_appearance("update-all", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("wait"));
    dialog.set_close_response("wait");

    let parent_widget = parent.clone().upcast::<gtk::Widget>();
    dialog.connect_response(None, move |_dialog, response| match response {
        "deploy" => on_deploy(),
        "update-all" => on_update_all(),
        _ => {
            if let Some(root) = parent_widget.root() {
                if let Some(window) = root.downcast_ref::<gtk::Window>() {
                    window.close();
                }
            }
        }
    });

    dialog.present(Some(parent));
}
