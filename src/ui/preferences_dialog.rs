use adw::prelude::*;
use gettextrs::gettext;

/// Present the application preferences dialog anchored at `parent`.
///
/// Currently hosts a single "Plugins" section listing every discovered plugin
/// descriptor with an enable switch. Toggling a switch persists the choice to
/// `AppConfig::disabled_plugins`; it takes effect on the next launch.
pub fn show_preferences_dialog(parent: &impl IsA<gtk::Widget>) {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title(&gettext("Preferences"));

    let page = adw::PreferencesPage::builder()
        .title(gettext("General"))
        .icon_name("preferences-system-symbolic")
        .build();

    let group = adw::PreferencesGroup::builder()
        .title(gettext("Plugins"))
        .description(gettext(
            "Backend plugins discovered from YAML descriptors in the system and \
             user data directories. Changes take effect after restarting Up.",
        ))
        .build();

    let disabled = crate::config::load_config().disabled_plugins;
    let descriptors = crate::plugins::discovery::discover_plugins();

    if descriptors.is_empty() {
        let empty = adw::ActionRow::builder()
            .title(gettext("No plugins found"))
            .subtitle(gettext(
                "Drop a descriptor in ~/.local/share/up/backends.d/ to add one",
            ))
            .build();
        empty.add_css_class("dim-label");
        group.add(&empty);
    } else {
        for descriptor in descriptors {
            let id = descriptor.id.clone();
            let row = adw::SwitchRow::builder()
                .title(&descriptor.display_name)
                .subtitle(&descriptor.description)
                .active(!disabled.iter().any(|d| d == &id))
                .build();
            row.connect_active_notify(move |row| {
                let mut cfg = crate::config::load_config();
                let active = row.is_active();
                let present = cfg.disabled_plugins.iter().any(|d| d == &id);
                if active && present {
                    cfg.disabled_plugins.retain(|d| d != &id);
                } else if !active && !present {
                    cfg.disabled_plugins.push(id.clone());
                } else {
                    return;
                }
                let _ = crate::config::save_config(&cfg);
            });
            group.add(&row);
        }
    }

    page.add(&group);
    dialog.add(&page);
    dialog.present(Some(parent));
}
