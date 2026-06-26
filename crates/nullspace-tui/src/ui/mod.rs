pub mod browser;
pub mod editor;
pub mod widgets;

use crate::app::{AppState, LayoutOrientation, Mode};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

pub const PREVIEW_PERCENT: u16 = 50;
pub const PREVIEW_VERTICAL_ROWS: u16 = 7;

pub fn draw(frame: &mut Frame<'_>, app: &mut AppState) {
    let cmdline_area = primary_pane_area(frame.area(), app.layout);
    match app.mode {
        Mode::Browser | Mode::Search | Mode::ConfirmDelete(_) => {
            widgets::clear_cmdline_overlay(frame, cmdline_area);
            browser::draw(frame, app);
        }
        Mode::Trash | Mode::ConfirmPurge(_) => {
            widgets::clear_cmdline_overlay(frame, cmdline_area);
            widgets::trash(frame, app);
            if let Mode::ConfirmPurge(id) = app.mode {
                let name = app
                    .trash_items
                    .iter()
                    .find(|item| item.id == id)
                    .map(|item| item.name.as_str())
                    .unwrap_or("selected equation");
                widgets::confirm_overlay(
                    frame,
                    "Confirm",
                    format!(
                        "Permanently delete \"{name}\"? (y/d/enter to confirm, n/esc to cancel)"
                    ),
                );
            }
        }
        Mode::Cmdline => {
            browser::draw(frame, app);
            widgets::cmdline(frame, cmdline_area, app);
        }
        Mode::Editor
        | Mode::RelatedPicker
        | Mode::ConfirmRemoveRelated(_)
        | Mode::ReferenceEditor
        | Mode::ConfirmRemoveReference(_) => {
            widgets::clear_cmdline_overlay(frame, cmdline_area);
            editor::draw(frame, app);
        }
    }
    widgets::notification(frame, app);
}

pub fn primary_pane_area(area: Rect, orientation: LayoutOrientation) -> Rect {
    let content = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area)[0];
    content_panes(content, orientation).0
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
    use super::{content_panes, primary_pane_area, PREVIEW_PERCENT, PREVIEW_VERTICAL_ROWS};
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

    #[test]
    fn primary_pane_area_excludes_preview_and_status_bar() {
        let area = Rect::new(0, 0, 100, 30);
        let primary = primary_pane_area(area, LayoutOrientation::Horizontal);

        assert_eq!(primary, Rect::new(0, 0, 100 - PREVIEW_PERCENT, 29));
    }
}
