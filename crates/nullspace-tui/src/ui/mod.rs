pub mod browser;
pub mod editor;
pub mod widgets;

use crate::app::{AppState, LayoutOrientation, Mode};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

pub const PREVIEW_PERCENT: u16 = 50;
pub const PREVIEW_VERTICAL_ROWS: u16 = 7;

pub fn draw(frame: &mut Frame<'_>, app: &mut AppState) {
    match app.mode {
        Mode::Browser | Mode::Search | Mode::ConfirmDelete(_) => browser::draw(frame, app),
        Mode::Editor
        | Mode::RelatedPicker
        | Mode::ConfirmRemoveRelated(_)
        | Mode::ReferenceEditor
        | Mode::ConfirmRemoveReference(_) => editor::draw(frame, app),
    }
    widgets::notification(frame, app);
}

pub fn content_panes(area: Rect, orientation: LayoutOrientation) -> (Rect, Rect) {
    match orientation {
        LayoutOrientation::Horizontal => {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(100 - PREVIEW_PERCENT),
                    Constraint::Percentage(PREVIEW_PERCENT),
                ])
                .split(area);
            (chunks[0], chunks[1])
        }
        LayoutOrientation::Vertical => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(PREVIEW_VERTICAL_ROWS),
                    Constraint::Min(1),
                ])
                .split(area);
            (chunks[1], chunks[0])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{content_panes, PREVIEW_VERTICAL_ROWS};
    use crate::app::LayoutOrientation;
    use ratatui::layout::Rect;

    #[test]
    fn vertical_layout_puts_preview_on_top_with_fixed_height() {
        let area = Rect::new(0, 0, 80, 40);
        let (primary, preview) = content_panes(area, LayoutOrientation::Vertical);

        assert_eq!(preview.y, 0);
        assert_eq!(preview.height, PREVIEW_VERTICAL_ROWS);
        assert_eq!(primary.y, PREVIEW_VERTICAL_ROWS);
        assert_eq!(preview.width, 80);
        assert_eq!(primary.width, 80);
    }

    #[test]
    fn horizontal_layout_is_side_by_side() {
        let area = Rect::new(0, 0, 80, 40);
        let (primary, preview) = content_panes(area, LayoutOrientation::Horizontal);

        assert_eq!(primary.height, 40);
        assert_eq!(preview.height, 40);
        assert!(preview.x >= primary.x + primary.width - 1);
    }
}
