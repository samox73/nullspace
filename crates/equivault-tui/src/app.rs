use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::{Duration, Instant};

use equivault_core::{
    render::render_image, Equation, EquationId, EquationSummary, Reference, Store, Variable,
};
use image::RgbaImage;

use crate::action::Action;
use crate::render_worker::{RenderJob, RenderWorker};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Browser,
    Editor,
    RelatedPicker,
    ConfirmDelete(EquationId),
    ConfirmRemoveRelated(EquationId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    List,
    Preview,
}

#[derive(Clone)]
pub struct EditorState {
    pub editing: Option<EquationId>,
    pub fields: [String; 7],
    pub cursors: [usize; 7],
    pub focus: usize,
    pub related_cursor: usize,
    pub dirty: bool,
    pub last_change: Instant,
    pub last_saved_signature: String,
    pub related_picker: RelatedPickerState,
}

#[derive(Clone)]
pub struct RelatedPickerState {
    pub cursor: usize,
    pub selected: Vec<EquationId>,
    pub query: String,
}

pub struct AppState {
    pub store: Store,
    pub mode: Mode,
    pub items: Vec<EquationSummary>,
    pub cursor: usize,
    pub focus: Pane,
    pub should_quit: bool,
    pub graphics_ok: bool,
    pub status: String,
    pub selected: Option<Equation>,
    pub editor: Option<EditorState>,
    pub preview: Option<RgbaImage>,
    pub preview_error: Option<String>,
    pub preview_latex: String,
    pub notification: Option<Notification>,
    editor_history: Vec<EditorState>,
    worker: RenderWorker,
    generation: u64,
    dispatched_generation: u64,
    last_change: Instant,
    cache: HashMap<u64, RgbaImage>,
    cache_order: VecDeque<u64>,
}

pub struct Notification {
    pub message: String,
    pub created_at: Instant,
}

impl AppState {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let mut store = Store::open(path)?;
        if store.list()?.is_empty() {
            seed(&mut store)?;
        }
        let mut app = Self {
            store,
            mode: Mode::Browser,
            items: Vec::new(),
            cursor: 0,
            focus: Pane::List,
            should_quit: false,
            graphics_ok: terminal_graphics_detected(),
            status: "Ready".to_string(),
            selected: None,
            editor: None,
            preview: None,
            preview_error: None,
            preview_latex: String::new(),
            notification: None,
            editor_history: Vec::new(),
            worker: RenderWorker::spawn(),
            generation: 0,
            dispatched_generation: 0,
            last_change: Instant::now(),
            cache: HashMap::new(),
            cache_order: VecDeque::new(),
        };
        app.reload()?;
        app.schedule_selected();
        Ok(app)
    }

    pub fn reload(&mut self) -> anyhow::Result<()> {
        self.items = self.store.list()?;
        if self.cursor >= self.items.len() {
            self.cursor = self.items.len().saturating_sub(1);
        }
        self.selected = self.selected_id().and_then(|id| self.store.get(id).ok());
        Ok(())
    }

    pub fn selected_id(&self) -> Option<EquationId> {
        self.items.get(self.cursor).map(|item| item.id)
    }

    pub fn apply(&mut self, action: Action) {
        let result: anyhow::Result<()> = (|| match action {
            Action::Quit => {
                self.should_quit = true;
                Ok(())
            }
            Action::MoveUp => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.selected = self.selected_id().and_then(|id| self.store.get(id).ok());
                    self.schedule_selected();
                }
                Ok(())
            }
            Action::MoveDown => {
                if self.cursor + 1 < self.items.len() {
                    self.cursor += 1;
                    self.selected = self.selected_id().and_then(|id| self.store.get(id).ok());
                    self.schedule_selected();
                }
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
            Action::NewEquation => {
                self.editor_history.clear();
                self.open_editor(None);
                Ok(())
            }
            Action::DeleteRequest => {
                if let Some(id) = self.selected_id() {
                    self.mode = Mode::ConfirmDelete(id);
                }
                Ok(())
            }
            Action::ConfirmYes => {
                if let Mode::ConfirmDelete(id) = self.mode {
                    self.store.delete(id)?;
                    self.reload()?;
                    self.mode = Mode::Browser;
                    self.status = "Deleted".to_string();
                    self.schedule_selected();
                }
                Ok(())
            }
            Action::ConfirmNo => {
                self.mode = Mode::Browser;
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
            Action::OpenCurrent => {
                if let Some(id) = self.selected_id() {
                    self.editor_history.clear();
                    self.open_editor(Some(id));
                }
                Ok(())
            }
            Action::Back => {
                self.mode = match self.mode {
                    Mode::ConfirmDelete(_) => Mode::Browser,
                    Mode::ConfirmRemoveRelated(_) => Mode::Editor,
                    Mode::Editor | Mode::RelatedPicker => {
                        if matches!(self.mode, Mode::Editor)
                            && self.editor.as_ref().is_some_and(|editor| editor.dirty)
                        {
                            self.persist_editor(false)?;
                        }
                        if let Some(previous) = self.editor_history.pop() {
                            self.editor = Some(previous);
                            self.schedule_selected();
                            Mode::Editor
                        } else {
                            self.editor = None;
                            Mode::Browser
                        }
                    }
                    Mode::Browser => Mode::Browser,
                };
                self.schedule_selected();
                Ok(())
            }
            Action::EditCurrent => {
                if let Some(id) = self.selected_id() {
                    self.editor_history.clear();
                    self.open_editor(Some(id));
                }
                Ok(())
            }
            Action::EditorNextField => {
                if let Some(editor) = &mut self.editor {
                    editor.focus = (editor.focus + 1) % editor.fields.len();
                }
                Ok(())
            }
            Action::EditorPrevField => {
                if let Some(editor) = &mut self.editor {
                    editor.focus = editor
                        .focus
                        .checked_sub(1)
                        .unwrap_or(editor.fields.len() - 1);
                }
                Ok(())
            }
            Action::EditorMoveLeft => {
                if let Some(editor) = &mut self.editor {
                    let field = editor.focus;
                    editor.cursors[field] =
                        prev_boundary(&editor.fields[field], editor.cursors[field]);
                }
                Ok(())
            }
            Action::EditorMoveRight => {
                if let Some(editor) = &mut self.editor {
                    let field = editor.focus;
                    editor.cursors[field] =
                        next_boundary(&editor.fields[field], editor.cursors[field]);
                }
                Ok(())
            }
            Action::EditorHome => {
                if let Some(editor) = &mut self.editor {
                    editor.cursors[editor.focus] = 0;
                }
                Ok(())
            }
            Action::EditorEnd => {
                if let Some(editor) = &mut self.editor {
                    editor.cursors[editor.focus] = editor.fields[editor.focus].len();
                }
                Ok(())
            }
            Action::EditorRelatedMoveUp => {
                if let Some(editor) = &mut self.editor {
                    if editor.focus == 6 {
                        editor.related_cursor = editor.related_cursor.saturating_sub(1);
                    }
                }
                Ok(())
            }
            Action::EditorRelatedMoveDown => {
                let max = self
                    .editor
                    .as_ref()
                    .map(|editor| {
                        parse_related(&editor.fields[6], &self.items)
                            .len()
                            .saturating_sub(1)
                    })
                    .unwrap_or(0);
                if let Some(editor) = &mut self.editor {
                    if editor.focus == 6 {
                        editor.related_cursor = (editor.related_cursor + 1).min(max);
                    }
                }
                Ok(())
            }
            Action::EditorSave => self.save_editor(),
            Action::EditorInput(key) => {
                self.input_editor(key);
                Ok(())
            }
            Action::RelatedPickerMoveUp => {
                if let Some(editor) = &mut self.editor {
                    editor.related_picker.cursor = editor.related_picker.cursor.saturating_sub(1);
                }
                Ok(())
            }
            Action::RelatedPickerMoveDown => {
                let max = self.filtered_related_picker_items().len().saturating_sub(1);
                if let Some(editor) = &mut self.editor {
                    editor.related_picker.cursor = (editor.related_picker.cursor + 1).min(max);
                }
                Ok(())
            }
            Action::RelatedPickerToggle => {
                self.toggle_related_picker_selection();
                Ok(())
            }
            Action::RelatedPickerApply => {
                self.apply_related_picker();
                Ok(())
            }
            Action::RelatedPickerCancel => {
                self.mode = Mode::Editor;
                Ok(())
            }
            Action::RelatedPickerInput(key) => {
                self.input_related_picker(key);
                Ok(())
            }
            Action::None => Ok(()),
        })();
        if let Err(err) = result {
            self.status = err.to_string();
        }
    }

    pub fn tick_render(&mut self) {
        if self
            .notification
            .as_ref()
            .is_some_and(|notification| notification.created_at.elapsed() >= Duration::from_secs(3))
        {
            self.notification = None;
        }

        while let Some(result) = self.worker.try_recv() {
            if result.generation < self.generation {
                continue;
            }
            match result.image {
                Ok(image) => {
                    self.preview_error = None;
                    self.remember_cache(hash_latex(&result.latex), image.clone());
                    self.preview = Some(image);
                }
                Err(err) => {
                    self.preview = None;
                    self.preview_error = Some(err);
                }
            }
        }

        if self.generation != self.dispatched_generation
            && self.last_change.elapsed() >= Duration::from_millis(150)
        {
            self.worker.send(RenderJob {
                generation: self.generation,
                latex: self.preview_latex.clone(),
                px: 48,
            });
            self.dispatched_generation = self.generation;
        }

        if matches!(self.mode, Mode::Editor)
            && self.editor.as_ref().is_some_and(|editor| {
                editor.dirty && editor.last_change.elapsed() >= Duration::from_millis(300)
            })
        {
            if let Err(err) = self.persist_editor(false) {
                self.status = err.to_string();
            }
        }
    }

    fn schedule_selected(&mut self) {
        let latex = if matches!(
            self.mode,
            Mode::Editor | Mode::RelatedPicker | Mode::ConfirmRemoveRelated(_)
        ) {
            self.editor
                .as_ref()
                .map(|editor| editor.fields[2].clone())
                .unwrap_or_default()
        } else {
            self.items
                .get(self.cursor)
                .map(|item| item.latex.clone())
                .unwrap_or_default()
        };
        self.schedule_latex(latex);
    }

    fn schedule_latex(&mut self, latex: String) {
        self.preview_latex = latex;
        self.generation = self.generation.saturating_add(1);
        self.last_change = Instant::now();
        let key = hash_latex(&self.preview_latex);
        if let Some(image) = self.cache.get(&key) {
            self.preview = Some(image.clone());
            self.preview_error = None;
            self.dispatched_generation = self.generation;
        }
    }

    fn open_editor(&mut self, id: Option<EquationId>) {
        let equation = id.and_then(|eq_id| self.store.get(eq_id).ok());
        let fields = if let Some(eq) = equation {
            [
                eq.name,
                eq.description,
                eq.latex,
                format_refs(&eq.references),
                eq.tags.join(", "),
                format_variables(&eq.variables),
                format_related(&eq.related, &self.items),
            ]
        } else {
            [
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ]
        };
        self.editor = Some(EditorState {
            editing: id,
            cursors: fields.each_ref().map(String::len),
            last_saved_signature: fields_signature(&fields),
            fields,
            focus: 0,
            related_cursor: 0,
            dirty: false,
            last_change: Instant::now(),
            related_picker: RelatedPickerState {
                cursor: 0,
                selected: Vec::new(),
                query: String::new(),
            },
        });
        self.mode = Mode::Editor;
        self.schedule_selected();
    }

    fn input_editor(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        let Some(editor) = &mut self.editor else {
            return;
        };
        let focused = editor.focus;
        let field = &mut editor.fields[focused];
        let cursor = editor.cursors[focused].min(field.len());
        match key.code {
            KeyCode::Char('r') if focused == 6 => {
                self.open_related_picker();
                return;
            }
            KeyCode::Char('k') if focused == 6 => {
                editor.related_cursor = editor.related_cursor.saturating_sub(1);
                return;
            }
            KeyCode::Char('j') if focused == 6 => {
                let max = parse_related(field, &self.items).len().saturating_sub(1);
                editor.related_cursor = (editor.related_cursor + 1).min(max);
                return;
            }
            KeyCode::Char('d') if focused == 6 => {
                if let Some(id) = self.current_related_id() {
                    self.mode = Mode::ConfirmRemoveRelated(id);
                }
                return;
            }
            KeyCode::Char(ch) => {
                if focused != 6 {
                    field.insert(cursor, ch);
                    editor.cursors[focused] = cursor + ch.len_utf8();
                }
            }
            KeyCode::Backspace => {
                if cursor > 0 {
                    let previous = prev_boundary(field, cursor);
                    field.drain(previous..cursor);
                    editor.cursors[focused] = previous;
                }
            }
            KeyCode::Delete => {
                if cursor < field.len() {
                    let next = next_boundary(field, cursor);
                    field.drain(cursor..next);
                    editor.cursors[focused] = cursor;
                }
            }
            KeyCode::Enter if matches!(focused, 1 | 2 | 3 | 5) => {
                field.insert(cursor, '\n');
                editor.cursors[focused] = cursor + 1;
            }
            KeyCode::Enter if focused == 6 => {
                self.open_selected_related_detail();
                return;
            }
            _ => {}
        }
        mark_editor_dirty(editor);
        if editor.focus == 2 {
            self.schedule_selected();
        }
    }

    pub fn filtered_related_picker_items(&self) -> Vec<&EquationSummary> {
        let Some(editor) = &self.editor else {
            return Vec::new();
        };
        related_picker_items_for(&self.items, editor.editing)
            .into_iter()
            .filter(|item| fuzzy_matches_item(&editor.related_picker.query, item))
            .collect()
    }

    fn open_related_picker(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        if editor.focus != 6 {
            return;
        }
        editor.related_picker.selected = parse_related(&editor.fields[6], &self.items);
        editor.related_picker.query.clear();
        let items = related_picker_items_for(&self.items, editor.editing);
        if editor.related_picker.cursor >= items.len() {
            editor.related_picker.cursor = items.len().saturating_sub(1);
        }
        self.mode = Mode::RelatedPicker;
    }

    fn toggle_related_picker_selection(&mut self) {
        let Some(item_id) = self
            .filtered_related_picker_items()
            .get(
                self.editor
                    .as_ref()
                    .map(|editor| editor.related_picker.cursor)
                    .unwrap_or(0),
            )
            .map(|item| item.id)
        else {
            return;
        };
        let Some(editor) = &mut self.editor else {
            return;
        };
        if let Some(index) = editor
            .related_picker
            .selected
            .iter()
            .position(|selected| selected == &item_id)
        {
            editor.related_picker.selected.remove(index);
        } else {
            editor.related_picker.selected.push(item_id);
        }
        mark_editor_dirty(editor);
    }

    fn input_related_picker(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        let Some(editor) = &mut self.editor else {
            return;
        };
        match key.code {
            KeyCode::Char(ch) => editor.related_picker.query.push(ch),
            KeyCode::Backspace => {
                editor.related_picker.query.pop();
            }
            KeyCode::Delete => editor.related_picker.query.clear(),
            _ => {}
        }
        let max = related_picker_items_for(&self.items, editor.editing)
            .into_iter()
            .filter(|item| fuzzy_matches_item(&editor.related_picker.query, item))
            .count()
            .saturating_sub(1);
        editor.related_picker.cursor = editor.related_picker.cursor.min(max);
    }

    fn apply_related_picker(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.fields[6] = format_related(&editor.related_picker.selected, &self.items);
        editor.cursors[6] = editor.fields[6].len();
        editor.related_cursor = editor
            .related_cursor
            .min(editor.related_picker.selected.len().saturating_sub(1));
        mark_editor_dirty(editor);
        self.mode = Mode::Editor;
    }

    fn open_selected_related_detail(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        if editor.focus != 6 {
            return;
        }
        let related = parse_related(&editor.fields[6], &self.items);
        let Some(id) = related.get(editor.related_cursor).copied() else {
            self.open_related_picker();
            return;
        };
        if let Some(current) = self.editor.clone() {
            self.editor_history.push(current);
            self.open_editor(Some(id));
        }
    }

    fn current_related_id(&self) -> Option<EquationId> {
        let editor = self.editor.as_ref()?;
        if editor.focus != 6 {
            return None;
        }
        parse_related(&editor.fields[6], &self.items)
            .get(editor.related_cursor)
            .copied()
    }

    fn remove_related_from_editor(&mut self, id: EquationId) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let mut related = parse_related(&editor.fields[6], &self.items);
        related.retain(|related_id| *related_id != id);
        editor.fields[6] = format_related(&related, &self.items);
        editor.cursors[6] = editor.fields[6].len();
        editor.related_cursor = editor.related_cursor.min(related.len().saturating_sub(1));
        mark_editor_dirty(editor);
    }

    fn save_editor(&mut self) -> anyhow::Result<()> {
        self.persist_editor(false)
    }

    fn persist_editor(&mut self, exit_after_save: bool) -> anyhow::Result<()> {
        let Some(editor) = &self.editor else {
            return Ok(());
        };
        if editor.fields[0].trim().is_empty() {
            return Ok(());
        }
        if editor.fields[2].trim().is_empty() {
            return Ok(());
        }
        let signature = fields_signature(&editor.fields);
        if signature == editor.last_saved_signature {
            if let Some(editor) = &mut self.editor {
                editor.dirty = false;
            }
            return Ok(());
        }
        render_image(&editor.fields[2], 48).map_err(anyhow::Error::msg)?;
        let mut equation = if let Some(id) = editor.editing {
            self.store.get(id)?
        } else {
            Equation::new(
                editor.fields[0].trim().to_string(),
                editor.fields[2].trim().to_string(),
            )
        };
        equation.name = editor.fields[0].trim().to_string();
        equation.description = editor.fields[1].trim().to_string();
        equation.latex = editor.fields[2].trim().to_string();
        equation.references = parse_refs(&editor.fields[3]);
        equation.tags = editor.fields[4]
            .split(',')
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        equation.variables = parse_variables(&editor.fields[5]);
        equation.related = parse_related(&editor.fields[6], &self.items);
        equation.updated_at = equivault_core::store::now_rfc3339();
        let saved_id = equation.id;
        if editor.editing.is_some() {
            self.store.update(&equation)?;
        } else {
            self.store.insert(&equation)?;
        }
        self.reload()?;
        if let Some(index) = self.items.iter().position(|item| item.id == saved_id) {
            self.cursor = index;
        }
        self.selected = self.store.get(saved_id).ok();
        if let Some(editor) = &mut self.editor {
            editor.editing = Some(saved_id);
            editor.dirty = false;
            editor.last_saved_signature = signature;
        }
        self.notification = Some(Notification {
            message: "equation saved".to_string(),
            created_at: Instant::now(),
        });
        self.status = "Saved".to_string();
        if exit_after_save {
            self.editor_history.clear();
            self.editor = None;
            self.mode = Mode::Browser;
        }
        self.schedule_selected();
        Ok(())
    }

    fn remember_cache(&mut self, key: u64, image: RgbaImage) {
        if !self.cache.contains_key(&key) {
            self.cache_order.push_back(key);
        }
        self.cache.insert(key, image);
        while self.cache_order.len() > 64 {
            if let Some(old) = self.cache_order.pop_front() {
                self.cache.remove(&old);
            }
        }
    }
}

fn seed(store: &mut Store) -> anyhow::Result<()> {
    let demos = [
        ("Mass energy equivalence", "E = mc^2"),
        (
            "Gauss law",
            "\\nabla \\cdot \\mathbf{E} = \\rho/\\varepsilon_0",
        ),
        ("Euler identity", "e^{i\\pi} + 1 = 0"),
    ];
    for (name, latex) in demos {
        let mut eq = Equation::new(name.to_string(), latex.to_string());
        eq.description = "Seed equation".to_string();
        store.insert(&eq)?;
    }
    Ok(())
}

fn hash_latex(latex: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    latex.hash(&mut hasher);
    hasher.finish()
}

fn format_refs(references: &[Reference]) -> String {
    references
        .iter()
        .map(|reference| match &reference.url {
            Some(url) if !url.is_empty() => format!("{} | {}", reference.text, url),
            _ => reference.text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_refs(raw: &str) -> Vec<Reference> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut parts = line.splitn(2, '|').map(str::trim);
            let text = parts.next().unwrap_or_default().to_string();
            let url = parts
                .next()
                .filter(|url| !url.is_empty())
                .map(ToOwned::to_owned);
            Reference { text, url }
        })
        .collect()
}

fn format_variables(variables: &[Variable]) -> String {
    variables
        .iter()
        .map(|variable| format!("{} = {}", variable.symbol, variable.description))
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_variables(raw: &str) -> Vec<Variable> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut parts = line.splitn(2, '=').map(str::trim);
            Variable {
                symbol: parts.next().unwrap_or_default().to_string(),
                description: parts.next().unwrap_or_default().to_string(),
            }
        })
        .collect()
}

fn format_related(related: &[EquationId], items: &[EquationSummary]) -> String {
    related
        .iter()
        .filter_map(|id| items.iter().find(|item| item.id == *id))
        .map(|item| item.name.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

fn parse_related(raw: &str, items: &[EquationSummary]) -> Vec<EquationId> {
    raw.split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .filter_map(|name| items.iter().find(|item| item.name == name))
        .map(|item| item.id)
        .collect()
}

fn fields_signature(fields: &[String; 7]) -> String {
    fields.join("\u{1f}")
}

fn mark_editor_dirty(editor: &mut EditorState) {
    editor.dirty = true;
    editor.last_change = Instant::now();
}

fn related_picker_items_for(
    items: &[EquationSummary],
    editing: Option<EquationId>,
) -> Vec<&EquationSummary> {
    items
        .iter()
        .filter(|item| Some(item.id) != editing)
        .collect()
}

fn fuzzy_matches_item(query: &str, item: &EquationSummary) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }
    let haystack = format!("{} {} {}", item.name, item.description, item.latex).to_lowercase();
    let needle = query.to_lowercase();
    haystack.contains(&needle) || fuzzy_subsequence(&needle, &haystack)
}

fn fuzzy_subsequence(needle: &str, haystack: &str) -> bool {
    let mut chars = needle.chars();
    let Some(mut wanted) = chars.next() else {
        return true;
    };
    for ch in haystack.chars() {
        if ch == wanted {
            match chars.next() {
                Some(next) => wanted = next,
                None => return true,
            }
        }
    }
    false
}

fn terminal_graphics_detected() -> bool {
    std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var_os("WEZTERM_PANE").is_some()
        || std::env::var("TERM")
            .map(|term| {
                term.contains("kitty") || term.contains("wezterm") || term.contains("xterm-kitty")
            })
            .unwrap_or(false)
        || std::env::var("TERM_PROGRAM")
            .map(|program| program.contains("iTerm") || program.contains("Ghostty"))
            .unwrap_or(false)
}

fn prev_boundary(value: &str, cursor: usize) -> usize {
    value[..cursor]
        .char_indices()
        .last()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_boundary(value: &str, cursor: usize) -> usize {
    value[cursor..]
        .char_indices()
        .nth(1)
        .map(|(index, _)| cursor + index)
        .unwrap_or(value.len())
}
