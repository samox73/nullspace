use nullspace_core::Error;
use tui_textarea::CursorMove;

use crate::action::Action;

use super::{
    AppState, BrowserFilter, BrowserFilterFocus, CmdlineState, EditorField, LayoutOrientation,
    Mode, Pane, QuantityFormState, TagPickerRow, quantity_label,
};

impl AppState {
    pub fn apply(&mut self, action: Action) {
        if !matches!(action, Action::StartGoPrefix) {
            self.vim_go_prefix = false;
        }
        let result: anyhow::Result<()> = (|| match action {
            Action::Quit => {
                self.should_quit = true;
                Ok(())
            }
            Action::OpenHelp => {
                self.help_open = true;
                Ok(())
            }
            Action::CloseHelp => {
                self.help_open = false;
                Ok(())
            }
            Action::MoveUp => {
                self.move_browser_cursor_to(self.cursor.saturating_sub(1));
                Ok(())
            }
            Action::MoveDown => {
                if !self.items.is_empty() {
                    self.move_browser_cursor_to(self.cursor + 1);
                }
                Ok(())
            }
            Action::MoveToTop => {
                self.move_browser_cursor_to(0);
                Ok(())
            }
            Action::MoveToBottom => {
                self.move_browser_cursor_to(self.items.len().saturating_sub(1));
                Ok(())
            }
            Action::StartGoPrefix => {
                self.vim_go_prefix = true;
                Ok(())
            }
            Action::FocusLeft => {
                self.focus = Pane::List;
                Ok(())
            }
            Action::FocusRight => {
                self.focus = Pane::Preview;
                Ok(())
            }
            Action::ToggleLayout => {
                self.layout = match self.layout {
                    LayoutOrientation::Horizontal => LayoutOrientation::Vertical,
                    LayoutOrientation::Vertical => LayoutOrientation::Horizontal,
                };
                Ok(())
            }
            Action::NewEquation => {
                self.clear_nav();
                self.open_editor(None);
                Ok(())
            }
            Action::CopyCurrent => self.copy_current_equation(),
            Action::CopyLatexToClipboard => self.copy_selected_latex_to_clipboard(),
            Action::OpenReference => self.open_reference(),
            Action::OpenEquations => {
                self.clear_nav();
                self.clear_browser_filter()?;
                self.mode = Mode::Browser;
                Ok(())
            }
            Action::OpenTags => {
                self.tag_picker_cursor = 0;
                self.tag_picker_scroll_offset = 0;
                self.mode = Mode::TagPicker;
                self.status = format!("Tags: {} tag(s)", self.tag_counts.len());
                Ok(())
            }
            Action::OpenQuantities => {
                self.push_nav();
                self.quantities = self.store.quantities()?;
                self.quantity_cursor = 0;
                self.quantity_scroll_offset = 0;
                self.mode = Mode::QuantityPicker;
                self.status = format!("Quantities: {} item(s)", self.quantities.len());
                Ok(())
            }
            Action::OpenTrash => {
                self.reload_trash()?;
                self.trash_cursor = 0;
                self.mode = Mode::Trash;
                self.status = format!("Trash: {} item(s)", self.trash_items.len());
                Ok(())
            }
            Action::TrashMoveUp => {
                self.trash_cursor = self.trash_cursor.saturating_sub(1);
                Ok(())
            }
            Action::TrashMoveDown => {
                let max = self.trash_items.len().saturating_sub(1);
                self.trash_cursor = (self.trash_cursor + 1).min(max);
                Ok(())
            }
            Action::TrashMoveToTop => {
                self.trash_cursor = 0;
                Ok(())
            }
            Action::TrashMoveToBottom => {
                self.trash_cursor = self.trash_items.len().saturating_sub(1);
                Ok(())
            }
            Action::TrashRestore => {
                if let Some(id) = self.selected_trash_id() {
                    match self.store.restore(id) {
                        Ok(()) => {
                            self.reload()?;
                            self.reload_trash()?;
                            self.status = "Restored".to_string();
                            self.schedule_selected();
                        }
                        Err(Error::Duplicate(_)) => {
                            self.status = "Can't restore: conflicting equation exists".to_string();
                        }
                        Err(err) => return Err(err.into()),
                    }
                }
                Ok(())
            }
            Action::TrashPurgeRequest => {
                if let Some(id) = self.selected_trash_id() {
                    self.mode = Mode::ConfirmPurge(id);
                }
                Ok(())
            }
            Action::TagPickerMoveUp => {
                self.move_tag_picker_cursor_to(self.tag_picker_cursor.saturating_sub(1));
                Ok(())
            }
            Action::TagPickerMoveDown => {
                self.move_tag_picker_cursor_to(self.tag_picker_cursor + 1);
                Ok(())
            }
            Action::TagPickerMoveToTop => {
                self.move_tag_picker_cursor_to(0);
                Ok(())
            }
            Action::TagPickerMoveToBottom => {
                self.move_tag_picker_cursor_to(self.tag_picker_rows().len().saturating_sub(1));
                Ok(())
            }
            Action::TagPickerApply => {
                let rows = self.tag_picker_rows();
                if let Some(row) = rows.get(self.tag_picker_cursor) {
                    self.browser_filter = match row {
                        TagPickerRow::Untagged { .. } => BrowserFilter::Untagged,
                        TagPickerRow::Tag { name, .. } => BrowserFilter::Tag(name.clone()),
                    };
                    self.cursor = 0;
                    self.refresh_items()?;
                    self.status =
                        format!("{}: {} match(es)", self.browser_title(), self.items.len());
                    self.schedule_selected();
                }
                self.mode = Mode::Browser;
                Ok(())
            }
            Action::TagPickerCancel => {
                self.mode = Mode::Browser;
                self.schedule_selected();
                Ok(())
            }
            Action::QuantityPickerMoveUp => {
                self.move_quantity_cursor_to(self.quantity_cursor.saturating_sub(1));
                Ok(())
            }
            Action::QuantityPickerMoveDown => {
                self.move_quantity_cursor_to(self.quantity_cursor + 1);
                Ok(())
            }
            Action::QuantityPickerMoveToTop => {
                self.move_quantity_cursor_to(0);
                Ok(())
            }
            Action::QuantityPickerMoveToBottom => {
                self.move_quantity_cursor_to(self.quantities.len().saturating_sub(1));
                Ok(())
            }
            Action::QuantityPickerApply => {
                if let Some((id, label)) = self
                    .quantities
                    .get(self.quantity_cursor)
                    .map(|(quantity, _)| (quantity.id, quantity_label(quantity)))
                {
                    self.push_nav();
                    self.open_quantity_equations(id, label)?;
                }
                self.mode = Mode::Browser;
                Ok(())
            }
            Action::QuantityPickerNew => {
                self.quantity_form = Some(QuantityFormState::empty());
                self.mode = Mode::QuantityForm;
                Ok(())
            }
            Action::QuantityPickerEdit => {
                if let Some((quantity, _)) = self.quantities.get(self.quantity_cursor) {
                    self.quantity_form = Some(QuantityFormState::from_quantity(quantity));
                    self.mode = Mode::QuantityForm;
                }
                Ok(())
            }
            Action::QuantityPickerDeleteRequest => {
                if let Some((quantity, _)) = self.quantities.get(self.quantity_cursor) {
                    self.mode = Mode::ConfirmRemoveQuantity(quantity.id);
                }
                Ok(())
            }
            Action::QuantityPickerBack => {
                if !self.restore_nav()? {
                    self.mode = Mode::Browser;
                    self.schedule_selected();
                }
                Ok(())
            }
            Action::DeleteRequest => {
                if let Some(id) = self.selected_id() {
                    self.mode = Mode::ConfirmDelete(id);
                }
                Ok(())
            }
            Action::ScanOpen => {
                self.start_scan(self.scan_agent);
                Ok(())
            }
            Action::ScanPaste => {
                self.scan_paste();
                Ok(())
            }
            Action::ScanCycleModel => {
                self.scan_cycle_model();
                Ok(())
            }
            Action::ScanCycleEffort => {
                self.scan_cycle_effort();
                Ok(())
            }
            Action::Rescan => self.rescan(),
            Action::OpenCmdline => {
                self.cmdline = Some(CmdlineState {
                    input: String::new(),
                    cursor: 0,
                    selected: 0,
                    return_mode: self.mode,
                });
                self.mode = Mode::Cmdline;
                Ok(())
            }
            Action::CmdlineInput(key) => {
                self.input_cmdline(key);
                Ok(())
            }
            Action::CmdlineAccept => {
                self.accept_cmdline();
                Ok(())
            }
            Action::CmdlineExecute => {
                self.execute_cmdline();
                Ok(())
            }
            Action::CmdlineCancel => {
                let return_mode = self
                    .cmdline
                    .as_ref()
                    .map(|cmdline| cmdline.return_mode)
                    .unwrap_or(Mode::Browser);
                self.cmdline = None;
                self.mode = return_mode;
                self.force_preview_redraw();
                Ok(())
            }
            Action::StartSearch => {
                self.browser_filter = BrowserFilter::Search(String::new());
                self.browser_filter_cursor = 0;
                self.browser_filter_focus = BrowserFilterFocus::Search;
                self.mode = Mode::Search;
                self.refresh_items()?;
                self.status = "Search".to_string();
                self.schedule_selected();
                Ok(())
            }
            Action::BrowserFilterInput(key) => {
                self.input_browser_filter(key)?;
                Ok(())
            }
            Action::BrowserFilterAccept => {
                self.mode = Mode::Browser;
                Ok(())
            }
            Action::BrowserFilterCancel | Action::ClearFilter => {
                self.clear_browser_filter()?;
                self.mode = Mode::Browser;
                Ok(())
            }
            Action::BrowserFilterToggleFocus => {
                self.browser_filter_focus = match self.browser_filter_focus {
                    BrowserFilterFocus::Search => BrowserFilterFocus::List,
                    BrowserFilterFocus::List => BrowserFilterFocus::Search,
                };
                Ok(())
            }
            Action::ConfirmYes => {
                if let Mode::ConfirmDelete(id) = self.mode {
                    self.store.trash(id)?;
                    self.reload()?;
                    self.mode = Mode::Browser;
                    self.status = "Moved to trash".to_string();
                    self.schedule_selected();
                }
                Ok(())
            }
            Action::ConfirmNo => {
                self.mode = Mode::Browser;
                Ok(())
            }
            Action::ConfirmPurgeYes => {
                if let Mode::ConfirmPurge(id) = self.mode {
                    self.store.purge(id)?;
                    self.reload_trash()?;
                    self.mode = if self.trash_items.is_empty() {
                        Mode::Browser
                    } else {
                        Mode::Trash
                    };
                    self.status = "Permanently deleted".to_string();
                }
                Ok(())
            }
            Action::ConfirmPurgeNo => {
                self.mode = Mode::Trash;
                Ok(())
            }
            Action::ConfirmQuantityRemoveYes => {
                if let Mode::ConfirmRemoveQuantity(id) = self.mode {
                    self.store.delete_quantity(id)?;
                    if matches!(self.browser_filter, BrowserFilter::Quantity { id: active, .. } if active == id)
                    {
                        self.browser_filter = BrowserFilter::None;
                    }
                    self.reload()?;
                    self.move_quantity_cursor_to(self.quantity_cursor);
                    self.mode = Mode::QuantityPicker;
                    self.status = "Quantity deleted".to_string();
                }
                Ok(())
            }
            Action::ConfirmQuantityRemoveNo => {
                self.mode = Mode::QuantityPicker;
                Ok(())
            }
            Action::ConfirmRelatedRemoveYes => {
                if let Mode::ConfirmRemoveRelated(id) = self.mode {
                    self.remove_related_from_editor(id);
                    self.mode = Mode::Editor;
                }
                Ok(())
            }
            Action::ConfirmRelatedRemoveNo => {
                self.mode = Mode::Editor;
                Ok(())
            }
            Action::ConfirmReferenceRemoveYes => {
                if let Mode::ConfirmRemoveReference(index) = self.mode {
                    self.remove_reference(index);
                    self.mode = Mode::Editor;
                }
                Ok(())
            }
            Action::ConfirmReferenceRemoveNo => {
                self.mode = Mode::Editor;
                Ok(())
            }
            Action::ConfirmVariableRemoveYes => {
                if let Mode::ConfirmRemoveVariable(index) = self.mode {
                    self.remove_variable(index);
                    self.mode = Mode::Editor;
                }
                Ok(())
            }
            Action::ConfirmVariableRemoveNo => {
                self.mode = Mode::Editor;
                Ok(())
            }
            Action::OpenCurrent => {
                if let Some(id) = self.selected_id() {
                    self.push_nav();
                    self.open_editor(Some(id));
                }
                Ok(())
            }
            Action::PreviewZoomIn => {
                self.adjust_zoom(true)?;
                Ok(())
            }
            Action::PreviewZoomOut => {
                self.adjust_zoom(false)?;
                Ok(())
            }
            Action::Back => {
                if matches!(self.mode, Mode::Editor)
                    && !self.scan_review
                    && self.editor.as_ref().is_some_and(|editor| editor.dirty)
                {
                    self.persist_editor(false)?;
                }
                if self.scan_review {
                    self.discard_scan();
                    return Ok(());
                }
                if matches!(self.mode, Mode::Scan) {
                    self.back_from_scan();
                    return Ok(());
                }
                if matches!(
                    self.mode,
                    Mode::Browser | Mode::Editor | Mode::QuantityPicker
                ) && self.restore_nav()?
                {
                    return Ok(());
                }
                self.mode = match self.mode {
                    Mode::ConfirmDelete(_) => Mode::Browser,
                    Mode::ConfirmPurge(_) => Mode::Trash,
                    Mode::ConfirmRemoveQuantity(_) => Mode::QuantityPicker,
                    Mode::ConfirmRemoveRelated(_) => Mode::Editor,
                    Mode::ConfirmRemoveReference(_) => Mode::Editor,
                    Mode::ConfirmRemoveVariable(_) => Mode::Editor,
                    Mode::ReferenceEditor => Mode::Editor,
                    Mode::VariableEditor => Mode::Editor,
                    Mode::Editor | Mode::RelatedPicker => {
                        self.editor = None;
                        Mode::Browser
                    }
                    Mode::Search | Mode::Cmdline | Mode::Browser => Mode::Browser,
                    Mode::Trash => Mode::Browser,
                    Mode::TagPicker => Mode::Browser,
                    Mode::QuantityPicker => Mode::Browser,
                    Mode::QuantityForm => Mode::QuantityPicker,
                    Mode::QuantityResolver => Mode::Editor,
                    Mode::Scan => Mode::Browser,
                };
                self.schedule_selected();
                Ok(())
            }
            Action::EditorNextField => {
                if let Some(editor) = &mut self.editor {
                    editor.focus = editor.focus.next();
                    editor.active = false;
                }
                Ok(())
            }
            Action::EditorPrevField => {
                if let Some(editor) = &mut self.editor {
                    editor.focus = editor.focus.prev();
                    editor.active = false;
                }
                Ok(())
            }
            Action::EditorActivateField => {
                if let Some(editor) = &mut self.editor {
                    editor.active = true;
                }
                Ok(())
            }
            Action::EditorDeactivateField => {
                if let Some(editor) = &mut self.editor {
                    editor.active = false;
                }
                Ok(())
            }
            Action::EditorMoveLeft => {
                if let Some(editor) = &mut self.editor
                    && !editor.focus.is_list()
                {
                    editor.field_mut(editor.focus).move_cursor(CursorMove::Back);
                }
                Ok(())
            }
            Action::EditorMoveRight => {
                if let Some(editor) = &mut self.editor
                    && !editor.focus.is_list()
                {
                    editor
                        .field_mut(editor.focus)
                        .move_cursor(CursorMove::Forward);
                }
                Ok(())
            }
            Action::EditorHome => {
                if let Some(editor) = &mut self.editor
                    && !editor.focus.is_list()
                {
                    editor.field_mut(editor.focus).move_cursor(CursorMove::Head);
                }
                Ok(())
            }
            Action::EditorEnd => {
                if let Some(editor) = &mut self.editor
                    && !editor.focus.is_list()
                {
                    editor.field_mut(editor.focus).move_cursor(CursorMove::End);
                }
                Ok(())
            }
            Action::EditorRelatedMoveUp => {
                if let Some(editor) = &mut self.editor {
                    match editor.focus {
                        EditorField::Related => {
                            editor.related_cursor = editor.related_cursor.saturating_sub(1)
                        }
                        EditorField::References => {
                            editor.reference_cursor = editor.reference_cursor.saturating_sub(1)
                        }
                        EditorField::Variables => {
                            editor.variable_cursor = editor.variable_cursor.saturating_sub(1)
                        }
                        field @ (EditorField::Name
                        | EditorField::Description
                        | EditorField::Latex
                        | EditorField::Assumptions
                        | EditorField::Tags) => editor.field_mut(field).move_cursor(CursorMove::Up),
                    }
                }
                Ok(())
            }
            Action::EditorRelatedMoveDown => {
                if let Some(editor) = &mut self.editor {
                    match editor.focus {
                        EditorField::Related => {
                            let max = editor.related.len().saturating_sub(1);
                            editor.related_cursor = (editor.related_cursor + 1).min(max);
                        }
                        EditorField::References => {
                            let max = editor.references.len().saturating_sub(1);
                            editor.reference_cursor = (editor.reference_cursor + 1).min(max);
                        }
                        EditorField::Variables => {
                            let max = editor.variables.len().saturating_sub(1);
                            editor.variable_cursor = (editor.variable_cursor + 1).min(max);
                        }
                        field @ (EditorField::Name
                        | EditorField::Description
                        | EditorField::Latex
                        | EditorField::Assumptions
                        | EditorField::Tags) => {
                            editor.field_mut(field).move_cursor(CursorMove::Down)
                        }
                    }
                }
                Ok(())
            }
            Action::EditorSave => {
                if self.scan_review {
                    self.confirm_scan()
                } else {
                    self.save_editor()
                }
            }
            Action::EditorInput(key) => {
                self.input_editor(key);
                Ok(())
            }
            Action::RelatedPickerMoveUp => {
                self.move_related_picker_cursor(false);
                Ok(())
            }
            Action::RelatedPickerMoveDown => {
                self.move_related_picker_cursor(true);
                Ok(())
            }
            Action::RelatedPickerToggle => {
                self.related_picker_space_or_toggle();
                Ok(())
            }
            Action::RelatedPickerToggleFocus => {
                self.toggle_related_picker_focus();
                Ok(())
            }
            Action::RelatedPickerApply => {
                self.apply_related_picker();
                Ok(())
            }
            Action::RelatedPickerCancel => {
                self.mode = Mode::Editor;
                self.schedule_selected();
                Ok(())
            }
            Action::RelatedPickerInput(key) => {
                self.input_related_picker(key);
                Ok(())
            }
            Action::ReferenceEditorNextField => {
                self.reference_form_next_field();
                Ok(())
            }
            Action::ReferenceEditorPrevField => {
                self.reference_form_prev_field();
                Ok(())
            }
            Action::ReferenceEditorSave => {
                self.save_reference_form();
                Ok(())
            }
            Action::ReferenceEditorCancel => {
                self.mode = Mode::Editor;
                Ok(())
            }
            Action::ReferenceEditorInput(key) => {
                self.input_reference_form(key);
                Ok(())
            }
            Action::VariableEditorNextField => {
                self.variable_form_next_field();
                Ok(())
            }
            Action::VariableEditorPrevField => {
                self.variable_form_prev_field();
                Ok(())
            }
            Action::VariableEditorSave => {
                self.save_variable_form();
                Ok(())
            }
            Action::VariableEditorCancel => {
                self.mode = Mode::Editor;
                Ok(())
            }
            Action::VariableEditorInput(key) => {
                self.input_variable_form(key);
                Ok(())
            }
            Action::QuantityFormNextField => {
                self.quantity_form_next_field();
                Ok(())
            }
            Action::QuantityFormPrevField => {
                self.quantity_form_prev_field();
                Ok(())
            }
            Action::QuantityFormSave => {
                self.save_quantity_form()?;
                Ok(())
            }
            Action::QuantityFormCancel => {
                self.quantity_form = None;
                self.mode = Mode::QuantityPicker;
                Ok(())
            }
            Action::QuantityFormInput(key) => {
                self.input_quantity_form(key);
                Ok(())
            }
            Action::ResolverMoveUp => {
                self.move_resolver_cursor(false);
                Ok(())
            }
            Action::ResolverMoveDown => {
                self.move_resolver_cursor(true);
                Ok(())
            }
            Action::ResolverAccept => self.accept_resolver(),
            Action::ResolverSkip => {
                if let Some(resolver) = &mut self.quantity_resolver {
                    resolver.skipped += 1;
                }
                self.advance_resolver();
                Ok(())
            }
            Action::ResolverInput(key) => {
                self.input_resolver(key);
                Ok(())
            }
            Action::None => Ok(()),
        })();
        if let Err(err) = result {
            self.report_error(err);
        }
    }
}
