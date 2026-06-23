pub mod browser;
pub mod editor;
pub mod widgets;

use crate::app::{AppState, Mode};
use ratatui::Frame;

pub fn draw(frame: &mut Frame<'_>, app: &mut AppState) {
    match app.mode {
        Mode::Browser | Mode::Search | Mode::VariableLookup | Mode::ConfirmDelete(_) => {
            browser::draw(frame, app)
        }
        Mode::Editor | Mode::RelatedPicker | Mode::ConfirmRemoveRelated(_) => {
            editor::draw(frame, app)
        }
    }
    widgets::notification(frame, app);
}
