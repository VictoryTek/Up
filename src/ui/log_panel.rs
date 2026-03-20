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
        let buffer = self.text_view.buffer();
        let mut end = buffer.end_iter();
        buffer.insert(&mut end, line);
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
