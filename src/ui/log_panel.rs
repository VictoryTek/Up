use gtk::prelude::*;

#[derive(Clone)]
pub struct LogPanel {
    pub expander: gtk::Expander,
    text_view: gtk::TextView,
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

        Self {
            expander,
            text_view,
        }
    }

    pub fn append_line(&self, line: &str) {
        let buffer = self.text_view.buffer();
        let mut end = buffer.end_iter();
        buffer.insert(&mut end, line);
        buffer.insert(&mut end, "\n");

        // Auto-scroll to bottom
        let mark = buffer.create_mark(None, &buffer.end_iter(), false);
        self.text_view.scroll_mark_onscreen(&mark);
        buffer.delete_mark(&mark);
    }

    pub fn clear(&self) {
        let buffer = self.text_view.buffer();
        buffer.set_text("");
    }
}
