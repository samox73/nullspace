use crate::action::Action;
use crate::app::{AppState, BrowserFilterFocus, Mode};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn map_key(key: KeyEvent, app: &AppState) -> Action {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Action::Quit;
    }
    match app.mode {
        Mode::Browser => match key.code {
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Char(':') => Action::OpenCmdline,
            KeyCode::Char('/') => Action::StartSearch,
            KeyCode::Esc => Action::ClearFilter,
            KeyCode::Char('j') | KeyCode::Down => Action::MoveDown,
            KeyCode::Char('k') | KeyCode::Up => Action::MoveUp,
            KeyCode::Char('h') => Action::FocusLeft,
            KeyCode::Char('l') => Action::FocusRight,
            KeyCode::Char('v') => Action::ToggleLayout,
            KeyCode::Char('n') => Action::NewEquation,
            KeyCode::Char('c') => Action::CopyCurrent,
            KeyCode::Char('y') => Action::CopyLatexToClipboard,
            KeyCode::Char('d') => Action::DeleteRequest,
            KeyCode::Char('+') | KeyCode::Char('=') => Action::PreviewZoomIn,
            KeyCode::Char('-') => Action::PreviewZoomOut,
            KeyCode::Enter => Action::OpenCurrent,
            _ => Action::None,
        },
        Mode::Cmdline => match key.code {
            KeyCode::Esc => Action::CmdlineCancel,
            KeyCode::Tab | KeyCode::BackTab => Action::CmdlineAccept,
            KeyCode::Enter => Action::CmdlineExecute,
            KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Char(_) => Action::CmdlineInput(key),
            _ => Action::None,
        },
        Mode::Search => match key.code {
            KeyCode::Esc => Action::BrowserFilterCancel,
            KeyCode::Tab | KeyCode::BackTab => Action::BrowserFilterToggleFocus,
            KeyCode::Enter => Action::BrowserFilterAccept,
            KeyCode::Char('j') | KeyCode::Down
                if app.browser_filter_focus == BrowserFilterFocus::List =>
            {
                Action::MoveDown
            }
            KeyCode::Char('k') | KeyCode::Up
                if app.browser_filter_focus == BrowserFilterFocus::List =>
            {
                Action::MoveUp
            }
            KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Char(_) => Action::BrowserFilterInput(key),
            _ => Action::None,
        },
        Mode::ConfirmDelete(_) => match key.code {
            KeyCode::Char('y') | KeyCode::Char('d') | KeyCode::Enter => Action::ConfirmYes,
            KeyCode::Char('n') | KeyCode::Esc => Action::ConfirmNo,
            _ => Action::None,
        },
        Mode::ConfirmRemoveRelated(_) => match key.code {
            KeyCode::Char('y') => Action::ConfirmRelatedRemoveYes,
            KeyCode::Char('n') | KeyCode::Esc => Action::ConfirmRelatedRemoveNo,
            _ => Action::None,
        },
        Mode::ConfirmRemoveReference(_) => match key.code {
            KeyCode::Char('y') | KeyCode::Char('d') | KeyCode::Enter => {
                Action::ConfirmReferenceRemoveYes
            }
            KeyCode::Char('n') | KeyCode::Esc => Action::ConfirmReferenceRemoveNo,
            _ => Action::None,
        },
        Mode::RelatedPicker => match key.code {
            KeyCode::Esc => Action::RelatedPickerCancel,
            KeyCode::Tab | KeyCode::BackTab => Action::RelatedPickerToggleFocus,
            KeyCode::Down => Action::RelatedPickerMoveDown,
            KeyCode::Up => Action::RelatedPickerMoveUp,
            KeyCode::Char(' ') => Action::RelatedPickerToggle,
            KeyCode::Enter => Action::RelatedPickerApply,
            KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Char(_) => Action::RelatedPickerInput(key),
            _ => Action::None,
        },
        Mode::Editor => {
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
                return Action::EditorSave;
            }
            match key.code {
                KeyCode::Esc => Action::Back,
                KeyCode::Tab => Action::EditorNextField,
                KeyCode::BackTab => Action::EditorPrevField,
                KeyCode::Up => Action::EditorRelatedMoveUp,
                KeyCode::Down => Action::EditorRelatedMoveDown,
                KeyCode::Left => Action::EditorMoveLeft,
                KeyCode::Right => Action::EditorMoveRight,
                KeyCode::Home => Action::EditorHome,
                KeyCode::End => Action::EditorEnd,
                _ => Action::EditorInput(key),
            }
        }
        Mode::ReferenceEditor => {
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
                return Action::ReferenceEditorSave;
            }
            match key.code {
                KeyCode::Esc => Action::ReferenceEditorCancel,
                KeyCode::Enter => Action::ReferenceEditorSave,
                KeyCode::Tab => Action::ReferenceEditorNextField,
                KeyCode::BackTab => Action::ReferenceEditorPrevField,
                _ => Action::ReferenceEditorInput(key),
            }
        }
    }
}
