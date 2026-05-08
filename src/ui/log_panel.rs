use adw::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

/// Maximum number of lines retained in the log panel buffer.
const LINE_CAP: i32 = 5_000;
/// Number of lines to evict from the top when the cap is exceeded.
const EVICT_BATCH: i32 = 100;

#[derive(Clone)]
pub struct LogPanel {
    pub expander: gtk::Expander,
    text_view: gtk::TextView,
    scroll_pending: Rc<Cell<bool>>,
    save_button: gtk::Button,
}

impl LogPanel {
    pub fn new() -> Self {
        let text_view = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(8)
            .right_margin(8)
            .css_classes(vec!["card"])
            .build();

        let scrolled = gtk::ScrolledWindow::builder()
            .min_content_height(200)
            .max_content_height(400)
            .child(&text_view)
            .build();

        // ToastOverlay wraps the scrolled window so toasts appear over the log text.
        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&scrolled));

        // Save button placed in the expander header.
        let save_button = gtk::Button::builder()
            .icon_name("document-save-symbolic")
            .tooltip_text(gettext("Save log to file"))
            .css_classes(vec!["flat", "circular"])
            .sensitive(false)
            .valign(gtk::Align::Center)
            .build();
        save_button.update_property(&[gtk::accessible::Property::Label(&gettext(
            "Save log to file",
        ))]);

        // Custom label widget: label text + save button in a horizontal box.
        let header_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let header_label = gtk::Label::new(Some(&gettext("Terminal Output")));
        header_box.append(&header_label);
        header_box.append(&save_button);

        let expander = gtk::Expander::builder()
            .label_widget(&header_box)
            .margin_top(12)
            .child(&toast_overlay)
            .build();

        let buffer = text_view.buffer();
        let end_iter = buffer.end_iter();
        buffer.create_mark(Some("scroll-end"), &end_iter, false);

        // Wire up the save button.
        {
            let text_view_weak = text_view.downgrade();
            let toast_overlay_clone = toast_overlay.clone();
            save_button.connect_clicked(move |_| {
                let Some(view) = text_view_weak.upgrade() else {
                    return;
                };
                let buffer = view.buffer();
                let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);

                if text.trim().is_empty() {
                    return;
                }

                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
                let secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let filename = format!("up-update-{secs}.log");
                let path = format!("{home}/{filename}");

                let toast_msg = match std::fs::write(&path, text.as_str()) {
                    Ok(_) => gettext("Log saved to ~/{}").replace("{}", &filename),
                    Err(e) => gettext("Failed to save log: {}").replace("{}", &e.to_string()),
                };

                toast_overlay_clone
                    .add_toast(adw::Toast::builder().title(&toast_msg).timeout(5).build());
            });
        }

        Self {
            expander,
            text_view,
            scroll_pending: Rc::new(Cell::new(false)),
            save_button,
        }
    }

    pub fn append_line(&self, line: &str) {
        let clean = strip_ansi(line);
        let buffer = self.text_view.buffer();
        let mut end = buffer.end_iter();
        buffer.insert(&mut end, &clean);
        buffer.insert(&mut end, "\n");

        // Enable the save button now that the log has content.
        if !self.save_button.is_sensitive() {
            self.save_button.set_sensitive(true);
        }

        // FIFO eviction: keep buffer at most LINE_CAP lines.
        if buffer.line_count() > LINE_CAP {
            let mut start = buffer.start_iter();
            if let Some(mut evict_end) = buffer.iter_at_line(EVICT_BATCH) {
                buffer.delete(&mut start, &mut evict_end);
            }
        }

        // Debounced scroll to bottom.
        self.schedule_scroll();
    }

    pub fn clear(&self) {
        let buffer = self.text_view.buffer();
        buffer.set_text("");
        self.save_button.set_sensitive(false);
    }

    /// Schedules a single scroll-to-bottom, coalescing rapid calls into one.
    fn schedule_scroll(&self) {
        if self.scroll_pending.get() {
            return;
        }
        self.scroll_pending.set(true);

        let pending = self.scroll_pending.clone();
        let weak_view = self.text_view.downgrade();

        glib::timeout_add_local_once(Duration::from_millis(80), move || {
            pending.set(false);
            if let Some(view) = weak_view.upgrade() {
                let buffer = view.buffer();
                if let Some(mark) = buffer.mark("scroll-end") {
                    buffer.move_mark(&mark, &buffer.end_iter());
                    view.scroll_mark_onscreen(&mark);
                }
            }
        });
    }
}

/// Remove ANSI/VT100 escape sequences from `s`.
///
/// Handles:
/// - CSI sequences: ESC `[` followed by parameter bytes (`0x30–0x3F`),
///   intermediate bytes (`0x20–0x2F`), and a final byte (`0x40–0x7E`).
/// - Simple two-byte ESC sequences: ESC followed by any ASCII letter.
///
/// Any other byte sequence starting with ESC is passed through unchanged
/// rather than silently discarding legitimate content.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        // ESC seen — inspect next character.
        match chars.peek().copied() {
            Some('[') => {
                // CSI sequence: consume '[' and everything up to and including
                // the final byte (first byte in 0x40–0x7E range).
                chars.next(); // consume '['
                for ch in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&ch) {
                        break; // final byte consumed; sequence complete
                    }
                }
            }
            Some(ch) if ch.is_ascii_alphabetic() => {
                // Simple two-byte escape (e.g., ESC M for reverse index).
                chars.next(); // consume the letter
            }
            _ => {
                // Unrecognised; emit ESC as-is rather than silently dropping.
                out.push('\x1b');
            }
        }
    }
    out
}
