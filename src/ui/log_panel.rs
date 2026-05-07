use gtk::prelude::*;

#[derive(Clone)]
pub struct LogPanel {
    pub expander: gtk::Expander,
    text_view: gtk::TextView,
    scroll_mark: gtk::TextMark,
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

        let expander = gtk::Expander::builder()
            .label("Terminal Output")
            .margin_top(12)
            .child(&scrolled)
            .build();

        let buffer = text_view.buffer();
        let end_iter = buffer.end_iter();
        let scroll_mark = buffer.create_mark(Some("scroll-end"), &end_iter, false);

        Self {
            expander,
            text_view,
            scroll_mark,
        }
    }

    pub fn append_line(&self, line: &str) {
        let clean = strip_ansi(line);
        let buffer = self.text_view.buffer();
        let mut end = buffer.end_iter();
        buffer.insert(&mut end, &clean);
        buffer.insert(&mut end, "\n");

        // Auto-scroll to bottom
        buffer.move_mark(&self.scroll_mark, &buffer.end_iter());
        self.text_view.scroll_mark_onscreen(&self.scroll_mark);
    }

    pub fn clear(&self) {
        let buffer = self.text_view.buffer();
        buffer.set_text("");
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
