use tui_textarea::{TextArea, WrapMode};

pub(super) fn textarea_from_text(text: &str) -> TextArea<'static> {
    let lines = textarea_lines(text);
    let cursor = textarea_end_cursor(&lines);
    let mut textarea = TextArea::new(lines.clone());
    textarea.set_lines(lines, cursor);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    textarea
}

pub(super) fn set_textarea_text(textarea: &mut TextArea<'static>, text: String) {
    let lines = textarea_lines(&text);
    let cursor = textarea_end_cursor(&lines);
    textarea.set_lines(lines, cursor);
}

pub(super) fn textarea_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        vec![String::new()]
    } else {
        text.split('\n').map(ToOwned::to_owned).collect()
    }
}

pub(super) fn textarea_end_cursor(lines: &[String]) -> (usize, usize) {
    let row = lines.len().saturating_sub(1);
    let column = lines.last().map(|line| line.chars().count()).unwrap_or(0);
    (row, column)
}

pub(super) fn textarea_text(textarea: &TextArea<'_>) -> String {
    textarea.lines().join("\n")
}
