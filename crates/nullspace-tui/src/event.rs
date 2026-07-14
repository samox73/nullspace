use crate::action::Action;
use crate::app::{AppState, BrowserFilterFocus, EditorField, Mode};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn map_key(key: KeyEvent, app: &AppState) -> Action {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Action::Quit;
    }
    if app.help_open {
        return match key.code {
            KeyCode::Char('?') | KeyCode::Esc => Action::CloseHelp,
            _ => Action::None,
        };
    }
    if key.code == KeyCode::Char('?') {
        return Action::OpenHelp;
    }
    match app.mode {
        Mode::Browser => match key.code {
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Char(':') => Action::OpenCmdline,
            KeyCode::Char('/') => Action::StartSearch,
            KeyCode::Esc => Action::ClearFilter,
            KeyCode::Char('j') | KeyCode::Down => Action::MoveDown,
            KeyCode::Char('k') | KeyCode::Up => Action::MoveUp,
            KeyCode::Char('g') if app.vim_go_prefix => Action::MoveToTop,
            KeyCode::Char('g') => Action::StartGoPrefix,
            KeyCode::Char('G') => Action::MoveToBottom,
            KeyCode::Char('h') => Action::FocusLeft,
            KeyCode::Char('l') => Action::FocusRight,
            KeyCode::Char('v') => Action::ToggleLayout,
            KeyCode::Char('n') => Action::NewEquation,
            KeyCode::Char('c') => Action::CopyCurrent,
            KeyCode::Char('y') => Action::CopyLatexToClipboard,
            KeyCode::Char('o') => Action::OpenReference,
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
            KeyCode::Char('g')
                if app.browser_filter_focus == BrowserFilterFocus::List && app.vim_go_prefix =>
            {
                Action::MoveToTop
            }
            KeyCode::Char('g') if app.browser_filter_focus == BrowserFilterFocus::List => {
                Action::StartGoPrefix
            }
            KeyCode::Char('G') if app.browser_filter_focus == BrowserFilterFocus::List => {
                Action::MoveToBottom
            }
            KeyCode::Char('o') if app.browser_filter_focus == BrowserFilterFocus::List => {
                Action::OpenReference
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
        Mode::Trash => match key.code {
            KeyCode::Char('j') | KeyCode::Down => Action::TrashMoveDown,
            KeyCode::Char('k') | KeyCode::Up => Action::TrashMoveUp,
            KeyCode::Char('g') if app.vim_go_prefix => Action::TrashMoveToTop,
            KeyCode::Char('g') => Action::StartGoPrefix,
            KeyCode::Char('G') => Action::TrashMoveToBottom,
            KeyCode::Char('r') => Action::TrashRestore,
            KeyCode::Char('d') | KeyCode::Delete => Action::TrashPurgeRequest,
            KeyCode::Esc | KeyCode::Char('q') => Action::Back,
            _ => Action::None,
        },
        Mode::TagPicker => match key.code {
            KeyCode::Char('j') | KeyCode::Down => Action::TagPickerMoveDown,
            KeyCode::Char('k') | KeyCode::Up => Action::TagPickerMoveUp,
            KeyCode::Char('g') if app.vim_go_prefix => Action::TagPickerMoveToTop,
            KeyCode::Char('g') => Action::StartGoPrefix,
            KeyCode::Char('G') => Action::TagPickerMoveToBottom,
            KeyCode::Enter => Action::TagPickerApply,
            KeyCode::Esc | KeyCode::Char('q') => Action::TagPickerCancel,
            _ => Action::None,
        },
        Mode::QuantityPicker => match key.code {
            KeyCode::Char('j') | KeyCode::Down => Action::QuantityPickerMoveDown,
            KeyCode::Char('k') | KeyCode::Up => Action::QuantityPickerMoveUp,
            KeyCode::Char('g') if app.vim_go_prefix => Action::QuantityPickerMoveToTop,
            KeyCode::Char('g') => Action::StartGoPrefix,
            KeyCode::Char('G') => Action::QuantityPickerMoveToBottom,
            KeyCode::Enter => Action::QuantityPickerApply,
            KeyCode::Char('n') => Action::QuantityPickerNew,
            KeyCode::Char('e') => Action::QuantityPickerEdit,
            KeyCode::Char('d') => Action::QuantityPickerDeleteRequest,
            KeyCode::Esc | KeyCode::Char('q') => Action::QuantityPickerCancel,
            _ => Action::None,
        },
        Mode::ConfirmPurge(_) => match key.code {
            KeyCode::Char('y') | KeyCode::Char('d') | KeyCode::Enter => Action::ConfirmPurgeYes,
            KeyCode::Char('n') | KeyCode::Esc => Action::ConfirmPurgeNo,
            _ => Action::None,
        },
        Mode::ConfirmRemoveQuantity(_) => match key.code {
            KeyCode::Char('y') | KeyCode::Char('d') | KeyCode::Enter => {
                Action::ConfirmQuantityRemoveYes
            }
            KeyCode::Char('n') | KeyCode::Esc => Action::ConfirmQuantityRemoveNo,
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
        Mode::ConfirmRemoveVariable(_) => match key.code {
            KeyCode::Char('y') | KeyCode::Char('d') | KeyCode::Enter => {
                Action::ConfirmVariableRemoveYes
            }
            KeyCode::Char('n') | KeyCode::Esc => Action::ConfirmVariableRemoveNo,
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
                KeyCode::Char('o')
                    if app
                        .editor
                        .as_ref()
                        .is_some_and(|editor| editor.focus == EditorField::References) =>
                {
                    Action::OpenReference
                }
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
        Mode::VariableEditor => {
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
                return Action::VariableEditorSave;
            }
            match key.code {
                KeyCode::Esc => Action::VariableEditorCancel,
                KeyCode::Enter => Action::VariableEditorSave,
                KeyCode::Tab => Action::VariableEditorNextField,
                KeyCode::BackTab => Action::VariableEditorPrevField,
                _ => Action::VariableEditorInput(key),
            }
        }
        Mode::QuantityForm => {
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
                return Action::QuantityFormSave;
            }
            match key.code {
                KeyCode::Esc => Action::QuantityFormCancel,
                KeyCode::Enter => Action::QuantityFormSave,
                KeyCode::Tab => Action::QuantityFormNextField,
                KeyCode::BackTab => Action::QuantityFormPrevField,
                _ => Action::QuantityFormInput(key),
            }
        }
        Mode::QuantityResolver => match key.code {
            KeyCode::Esc => Action::ResolverSkip,
            KeyCode::Enter => Action::ResolverAccept,
            KeyCode::Down => Action::ResolverMoveDown,
            KeyCode::Up => Action::ResolverMoveUp,
            KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Char(_) => Action::ResolverInput(key),
            _ => Action::None,
        },
    }
}
