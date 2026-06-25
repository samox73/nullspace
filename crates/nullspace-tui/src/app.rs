use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::time::{Duration, Instant};

use image::RgbaImage;
use nullspace_core::reference::normalize_doi;
use nullspace_core::{
    render::validate_latex, Equation, EquationId, EquationSummary, Reference, Store, Variable,
};
use ratatui::layout::Size;
use ratatui_image::protocol::StatefulProtocol;
use tui_textarea::{CursorMove, TextArea, WrapMode};

use crate::action::Action;
use crate::graphics::{Graphics, TerminalCellSize};
use crate::protocol_warm_worker::{
    ProtocolWarmJob, ProtocolWarmOutcome, ProtocolWarmResult, ProtocolWarmSource,
    ProtocolWarmWorker,
};
use crate::render_cache;
use crate::render_worker::{RenderJob, RenderResult, RenderWorker};
use crate::warm_worker::{WarmJob, WarmOutcome, WarmResult, WarmWorker};

const IMAGE_CACHE_CAPACITY: usize = 128;
// Comfortably exceeds 2 * WARM_RADIUS so neighbours pre-encoded for both scroll
// directions (plus a little history) survive in the cache without thrashing.
const PROTOCOL_CACHE_CAPACITY: usize = 48;
const WARM_RADIUS: usize = 8;
const RELATED_PICKER_PREVIEW_PX: u32 = 512;
const RESULT_PULL_LIMIT: usize = 64;
const PROTOCOL_RESULTS_PER_TICK: usize = 16;
const WARM_RESULTS_PER_TICK: usize = 3;
const RENDER_RESULTS_PER_TICK: usize = 2;
const RESULT_TICK_BUDGET: Duration = Duration::from_millis(4);
const MIN_RENDER_PX: u32 = 16;
const MAX_RENDER_PX: u32 = 512;
const FALLBACK_TERMINAL_CELL_PX_HEIGHT: u32 = 26;
const PREVIEW_RENDER_EDGE_GUARD_PX: u32 = 0;
const DEFAULT_EQUATION_ROWS: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Browser,
    Search,
    Editor,
    RelatedPicker,
    ReferenceEditor,
    ConfirmDelete(EquationId),
    ConfirmRemoveRelated(EquationId),
    ConfirmRemoveReference(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    List,
    Preview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutOrientation {
    Horizontal,
    Vertical,
}

#[derive(Clone)]
pub struct EditorState {
    pub editing: Option<EquationId>,
    pub fields: [TextArea<'static>; 7],
    pub focus: usize,
    pub related_cursor: usize,
    pub reference_cursor: usize,
    pub references: Vec<Reference>,
    pub reference_form: ReferenceForm,
    pub dirty: bool,
    pub last_change: Instant,
    pub last_saved_signature: String,
    pub related_picker: RelatedPickerState,
    pub related: Vec<EquationId>,
}

impl EditorState {
    pub fn field_text(&self, index: usize) -> String {
        textarea_text(&self.fields[index])
    }

    fn field_texts(&self) -> [String; 7] {
        std::array::from_fn(|index| self.field_text(index))
    }

    fn set_field_text(&mut self, index: usize, text: String) {
        set_textarea_text(&mut self.fields[index], text);
    }
}

pub const REFERENCE_FIELD_LABELS: [&str; 5] = ["Authors", "Year", "Title", "DOI", "URL"];

#[derive(Clone)]
pub struct ReferenceForm {
    pub fields: [TextArea<'static>; 5],
    pub focus: usize,
    pub editing: Option<usize>,
    pub error: Option<String>,
}

impl ReferenceForm {
    fn empty() -> Self {
        Self {
            fields: std::array::from_fn(|_| textarea_from_text("")),
            focus: 0,
            editing: None,
            error: None,
        }
    }

    fn from_reference(reference: &Reference, index: usize) -> Self {
        let values = [
            reference.authors.clone(),
            reference.year.map(|y| y.to_string()).unwrap_or_default(),
            reference.title.clone(),
            reference.doi.clone().unwrap_or_default(),
            reference.url.clone().unwrap_or_default(),
        ];
        Self {
            fields: values.each_ref().map(|value| textarea_from_text(value)),
            focus: 0,
            editing: Some(index),
            error: None,
        }
    }

    fn field_text(&self, index: usize) -> String {
        textarea_text(&self.fields[index])
    }
}

#[derive(Clone)]
pub struct RelatedPickerState {
    pub cursor: usize,
    pub list_scroll_offset: usize,
    pub list_visible_height: u16,
    pub selected: Vec<EquationId>,
    pub query: String,
    pub query_cursor: usize,
    pub focus: RelatedPickerFocus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelatedPickerFocus {
    Search,
    List,
}

#[derive(Clone)]
pub enum BrowserFilter {
    None,
    Search(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserFilterFocus {
    Search,
    List,
}

pub struct AppState {
    pub store: Store,
    pub mode: Mode,
    pub all_items: Vec<EquationSummary>,
    pub items: Vec<EquationSummary>,
    pub browser_filter: BrowserFilter,
    pub browser_filter_cursor: usize,
    pub browser_filter_focus: BrowserFilterFocus,
    pub cursor: usize,
    pub list_scroll_offset: usize,
    pub list_visible_height: u16,
    pub focus: Pane,
    pub layout: LayoutOrientation,
    pub should_quit: bool,
    pub graphics_ok: bool,
    pub cell_size_px: TerminalCellSize,
    pub status: String,
    pub selected: Option<Equation>,
    pub editor: Option<EditorState>,
    pub preview_protocol: Option<StatefulProtocol>,
    pub preview_error: Option<String>,
    pub preview_latex: String,
    pub preview_px: u32,
    pub preview_render_px: u32,
    pub preview_preserve_on_error: bool,
    pub notification: Option<Notification>,
    editor_history: Vec<EditorState>,
    worker: RenderWorker,
    warm_worker: WarmWorker,
    protocol_warm_worker: ProtocolWarmWorker,
    pending_protocol_results: VecDeque<ProtocolWarmResult>,
    pending_warm_results: VecDeque<WarmResult>,
    pending_render_results: VecDeque<RenderResult>,
    generation: u64,
    dispatched_generation: u64,
    last_change: Instant,
    cache: HashMap<u64, RgbaImage>,
    cache_order: VecDeque<u64>,
    warm_inflight: HashSet<u64>,
    warm_failed: HashSet<u64>,
    protocol_warm_inflight: HashMap<u64, Size>,
    protocol_cache: HashMap<u64, StatefulProtocol>,
    protocol_cache_order: VecDeque<u64>,
    preview_cache_key: u64,
    preview_warm_size: Option<Size>,
    render_inflight_key: Option<u64>,
    graphics: Graphics,
}

pub struct Notification {
    pub message: String,
    pub created_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStatus {
    Cached,
    Loading,
    Empty,
}

impl AppState {
    pub fn open(path: &Path, graphics: Graphics) -> anyhow::Result<Self> {
        let mut store = Store::open(path)?;
        let cell_size_px = graphics.cell_size_px;
        let default_equation_px = default_equation_px(cell_size_px);
        if store.list()?.is_empty() {
            seed(&mut store, default_equation_px)?;
        }
        let graphics_ok = graphics.graphics_ok;
        let mut app = Self {
            store,
            mode: Mode::Browser,
            all_items: Vec::new(),
            items: Vec::new(),
            browser_filter: BrowserFilter::None,
            browser_filter_cursor: 0,
            browser_filter_focus: BrowserFilterFocus::Search,
            cursor: 0,
            list_scroll_offset: 0,
            list_visible_height: 0,
            focus: Pane::List,
            layout: LayoutOrientation::Vertical,
            should_quit: false,
            graphics_ok,
            cell_size_px,
            status: "Ready".to_string(),
            selected: None,
            editor: None,
            preview_protocol: None,
            preview_error: None,
            preview_latex: String::new(),
            preview_px: default_equation_px,
            preview_render_px: default_equation_px,
            preview_preserve_on_error: false,
            notification: None,
            editor_history: Vec::new(),
            worker: RenderWorker::spawn(),
            warm_worker: WarmWorker::spawn(),
            protocol_warm_worker: ProtocolWarmWorker::spawn(),
            pending_protocol_results: VecDeque::new(),
            pending_warm_results: VecDeque::new(),
            pending_render_results: VecDeque::new(),
            generation: 0,
            dispatched_generation: 0,
            last_change: Instant::now(),
            cache: HashMap::new(),
            cache_order: VecDeque::new(),
            warm_inflight: HashSet::new(),
            warm_failed: HashSet::new(),
            protocol_warm_inflight: HashMap::new(),
            protocol_cache: HashMap::new(),
            protocol_cache_order: VecDeque::new(),
            preview_cache_key: 0,
            preview_warm_size: None,
            render_inflight_key: None,
            graphics,
        };
        app.reload()?;
        app.schedule_selected();
        Ok(app)
    }

    pub fn reload(&mut self) -> anyhow::Result<()> {
        let selected = self.selected_id();
        self.all_items = self.store.list()?;
        self.refresh_items()?;
        if let Some(id) = selected {
            if let Some(index) = self.items.iter().position(|item| item.id == id) {
                self.cursor = index;
            }
        }
        if self.cursor >= self.items.len() {
            self.cursor = self.items.len().saturating_sub(1);
        }
        self.selected = self.selected_id().and_then(|id| self.store.get(id).ok());
        Ok(())
    }

    pub fn selected_id(&self) -> Option<EquationId> {
        self.items.get(self.cursor).map(|item| item.id)
    }

    pub fn browser_title(&self) -> String {
        match &self.browser_filter {
            BrowserFilter::None => "Equations".to_string(),
            BrowserFilter::Search(query) => format!("Search: {}", query),
        }
    }

    pub fn set_preview_warm_size(&mut self, size: Size) {
        if size.width == 0 || size.height == 0 || self.preview_warm_size == Some(size) {
            return;
        }
        self.preview_warm_size = Some(size);
        if !self.preview_latex.is_empty()
            && self.effective_render_px(self.preview_px) != self.preview_render_px
        {
            self.schedule_latex_inner(self.preview_latex.clone(), self.preview_px, true);
            return;
        }
        // The size only becomes known once the preview pane is first drawn. If the current
        // equation is sitting on a spinner with its image already decoded, kick off its
        // (async) encode now that we know the target size.
        if self.preview_protocol.is_none() {
            if let Some(display) = self.cache.get(&self.preview_cache_key).cloned() {
                self.queue_current_protocol_warm(self.preview_cache_key, display);
            }
        }
        if matches!(self.mode, Mode::Browser | Mode::Search) {
            self.schedule_warm_neighbors();
        }
    }

    pub fn cache_status_for(&self, latex: &str, px: u32) -> CacheStatus {
        let key = render_cache::key(latex, self.effective_render_px(px));
        if self.warm_inflight.contains(&key)
            || self.protocol_warm_inflight.contains_key(&key)
            || self.render_inflight_key == Some(key)
            || (key == self.preview_cache_key && self.generation != self.dispatched_generation)
        {
            CacheStatus::Loading
        } else if self.cache.contains_key(&key)
            || self.protocol_cache.contains_key(&key)
            || (key == self.preview_cache_key && self.preview_protocol.is_some())
        {
            CacheStatus::Cached
        } else {
            CacheStatus::Empty
        }
    }

    pub fn cache_spinner(&self) -> &'static str {
        const FRAMES: [&str; 4] = ["-", "\\", "|", "/"];
        let index = ((self.last_change.elapsed().as_millis() / 120) as usize) % FRAMES.len();
        FRAMES[index]
    }

    pub fn cursor_visible(&self) -> bool {
        (self.last_change.elapsed().as_millis() / 200).is_multiple_of(2)
    }

    fn refresh_items(&mut self) -> anyhow::Result<()> {
        self.items = match &self.browser_filter {
            BrowserFilter::None => self.all_items.clone(),
            BrowserFilter::Search(query) => self.store.search(query)?,
        };
        self.cursor = self.cursor.min(self.items.len().saturating_sub(1));
        self.list_scroll_offset = self.list_scroll_offset.min(self.cursor);
        self.selected = self.selected_id().and_then(|id| self.store.get(id).ok());
        Ok(())
    }

    fn clear_browser_filter(&mut self) -> anyhow::Result<()> {
        self.browser_filter = BrowserFilter::None;
        self.browser_filter_cursor = 0;
        self.browser_filter_focus = BrowserFilterFocus::Search;
        self.refresh_items()?;
        self.status = "Filter cleared".to_string();
        self.schedule_selected();
        Ok(())
    }

    fn input_browser_filter(&mut self, key: crossterm::event::KeyEvent) -> anyhow::Result<()> {
        use crossterm::event::KeyCode;
        let query = match &mut self.browser_filter {
            BrowserFilter::Search(query) => query,
            BrowserFilter::None => return Ok(()),
        };
        self.browser_filter_cursor = self.browser_filter_cursor.min(query.len());
        let mut changed = false;
        match key.code {
            KeyCode::Char(ch) => {
                query.insert(self.browser_filter_cursor, ch);
                self.browser_filter_cursor += ch.len_utf8();
                changed = true;
            }
            KeyCode::Backspace => {
                if self.browser_filter_cursor > 0 {
                    let previous = prev_boundary(query, self.browser_filter_cursor);
                    query.drain(previous..self.browser_filter_cursor);
                    self.browser_filter_cursor = previous;
                    changed = true;
                }
            }
            KeyCode::Delete => {
                if self.browser_filter_cursor < query.len() {
                    let next = next_boundary(query, self.browser_filter_cursor);
                    query.drain(self.browser_filter_cursor..next);
                    changed = true;
                }
            }
            KeyCode::Left => {
                self.browser_filter_cursor = prev_boundary(query, self.browser_filter_cursor);
            }
            KeyCode::Right => {
                self.browser_filter_cursor = next_boundary(query, self.browser_filter_cursor);
            }
            KeyCode::Home => self.browser_filter_cursor = 0,
            KeyCode::End => self.browser_filter_cursor = query.len(),
            _ => {}
        }
        if changed {
            self.cursor = 0;
            self.refresh_items()?;
            self.status = format!("Search: {} match(es)", self.items.len());
            self.schedule_selected();
        }
        Ok(())
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
                    if self.cursor < self.list_scroll_offset {
                        self.list_scroll_offset = self.cursor;
                    }
                    self.schedule_selected_deferred();
                }
                Ok(())
            }
            Action::MoveDown => {
                if self.cursor + 1 < self.items.len() {
                    self.cursor += 1;
                    let visible = list_visible_item_count(self.list_visible_height).max(1);
                    if self.cursor >= self.list_scroll_offset + visible {
                        self.list_scroll_offset = self.cursor + 1 - visible;
                    }
                    self.schedule_selected_deferred();
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
            Action::ToggleLayout => {
                self.layout = match self.layout {
                    LayoutOrientation::Horizontal => LayoutOrientation::Vertical,
                    LayoutOrientation::Vertical => LayoutOrientation::Horizontal,
                };
                Ok(())
            }
            Action::NewEquation => {
                self.editor_history.clear();
                self.open_editor(None);
                Ok(())
            }
            Action::CopyCurrent => self.copy_current_equation(),
            Action::CopyLatexToClipboard => self.copy_selected_latex_to_clipboard(),
            Action::DeleteRequest => {
                if let Some(id) = self.selected_id() {
                    self.mode = Mode::ConfirmDelete(id);
                }
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
            Action::OpenCurrent => {
                if let Some(id) = self.selected_id() {
                    self.editor_history.clear();
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
                self.mode = match self.mode {
                    Mode::ConfirmDelete(_) => Mode::Browser,
                    Mode::ConfirmRemoveRelated(_) => Mode::Editor,
                    Mode::ConfirmRemoveReference(_) => Mode::Editor,
                    Mode::ReferenceEditor => Mode::Editor,
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
                    Mode::Search | Mode::Browser => Mode::Browser,
                };
                self.schedule_selected();
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
                    if editor.focus != 6 && editor.focus != 3 {
                        editor.fields[editor.focus].move_cursor(CursorMove::Back);
                    }
                }
                Ok(())
            }
            Action::EditorMoveRight => {
                if let Some(editor) = &mut self.editor {
                    if editor.focus != 6 && editor.focus != 3 {
                        editor.fields[editor.focus].move_cursor(CursorMove::Forward);
                    }
                }
                Ok(())
            }
            Action::EditorHome => {
                if let Some(editor) = &mut self.editor {
                    if editor.focus != 6 && editor.focus != 3 {
                        editor.fields[editor.focus].move_cursor(CursorMove::Head);
                    }
                }
                Ok(())
            }
            Action::EditorEnd => {
                if let Some(editor) = &mut self.editor {
                    if editor.focus != 6 && editor.focus != 3 {
                        editor.fields[editor.focus].move_cursor(CursorMove::End);
                    }
                }
                Ok(())
            }
            Action::EditorRelatedMoveUp => {
                if let Some(editor) = &mut self.editor {
                    match editor.focus {
                        6 => editor.related_cursor = editor.related_cursor.saturating_sub(1),
                        3 => editor.reference_cursor = editor.reference_cursor.saturating_sub(1),
                        focus => editor.fields[focus].move_cursor(CursorMove::Up),
                    }
                }
                Ok(())
            }
            Action::EditorRelatedMoveDown => {
                if let Some(editor) = &mut self.editor {
                    match editor.focus {
                        6 => {
                            let max = editor.related.len().saturating_sub(1);
                            editor.related_cursor = (editor.related_cursor + 1).min(max);
                        }
                        3 => {
                            let max = editor.references.len().saturating_sub(1);
                            editor.reference_cursor = (editor.reference_cursor + 1).min(max);
                        }
                        focus => editor.fields[focus].move_cursor(CursorMove::Down),
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
            Action::None => Ok(()),
        })();
        if let Err(err) = result {
            self.status = err.to_string();
        }
    }

    pub fn tick_render(&mut self) {
        let started = Instant::now();
        if self
            .notification
            .as_ref()
            .is_some_and(|notification| notification.created_at.elapsed() >= Duration::from_secs(3))
        {
            self.notification = None;
        }

        self.collect_worker_results();
        self.process_current_preview_results();
        self.process_protocol_results(started);
        self.process_warm_results(started);
        self.process_render_results(started);

        if self.generation != self.dispatched_generation
            && self.last_change.elapsed() >= Duration::from_millis(150)
        {
            self.worker.send(RenderJob {
                generation: self.generation,
                latex: self.preview_latex.clone(),
                px: self.preview_render_px,
            });
            self.render_inflight_key = Some(render_cache::key(
                &self.preview_latex,
                self.preview_render_px,
            ));
            self.dispatched_generation = self.generation;
        }

        if matches!(self.mode, Mode::Editor)
            && self.editor.as_ref().is_some_and(|editor| {
                editor.dirty && editor.last_change.elapsed() >= Duration::from_millis(300)
            })
        {
            if let Err(err) = self.persist_editor(false) {
                self.status = err.to_string();
                if let Some(editor) = &mut self.editor {
                    editor.last_change = Instant::now();
                }
            }
        }
    }

    fn collect_worker_results(&mut self) {
        for _ in 0..RESULT_PULL_LIMIT {
            let Some(result) = self.protocol_warm_worker.try_recv() else {
                break;
            };
            self.pending_protocol_results.push_back(result);
        }
        for _ in 0..RESULT_PULL_LIMIT {
            let Some(result) = self.warm_worker.try_recv() else {
                break;
            };
            self.pending_warm_results.push_back(result);
        }
        for _ in 0..RESULT_PULL_LIMIT {
            let Some(result) = self.worker.try_recv() else {
                break;
            };
            self.pending_render_results.push_back(result);
        }
    }

    fn process_current_preview_results(&mut self) {
        let preview_key = self.preview_cache_key;
        if let Some(index) = self
            .pending_protocol_results
            .iter()
            .position(|result| protocol_result_key(result) == Some(preview_key))
        {
            if let Some(result) = self.pending_protocol_results.remove(index) {
                self.handle_protocol_result(result);
            }
        }
        if let Some(index) = self
            .pending_warm_results
            .iter()
            .position(|result| warm_result_key(result) == Some(preview_key))
        {
            if let Some(result) = self.pending_warm_results.remove(index) {
                self.handle_warm_result(result);
            }
        }
        if let Some(index) = self
            .pending_render_results
            .iter()
            .position(|result| render_cache::key(&result.latex, result.px) == preview_key)
        {
            if let Some(result) = self.pending_render_results.remove(index) {
                self.handle_render_result(result);
            }
        }
    }

    fn process_protocol_results(&mut self, started: Instant) {
        for _ in 0..PROTOCOL_RESULTS_PER_TICK {
            if result_budget_spent(started) {
                break;
            }
            let Some(result) = self.pending_protocol_results.pop_front() else {
                break;
            };
            self.handle_protocol_result(result);
        }
    }

    fn process_warm_results(&mut self, started: Instant) {
        for _ in 0..WARM_RESULTS_PER_TICK {
            if result_budget_spent(started) {
                break;
            }
            let Some(result) = self.pending_warm_results.pop_front() else {
                break;
            };
            self.handle_warm_result(result);
        }
    }

    fn process_render_results(&mut self, started: Instant) {
        for _ in 0..RENDER_RESULTS_PER_TICK {
            if result_budget_spent(started) {
                break;
            }
            let Some(result) = self.pending_render_results.pop_front() else {
                break;
            };
            self.handle_render_result(result);
        }
    }

    fn handle_protocol_result(&mut self, result: ProtocolWarmResult) {
        match result.outcome {
            ProtocolWarmOutcome::Ready {
                key,
                size,
                protocol,
            } => {
                self.remove_protocol_inflight(key, size);
                let is_current_size = self.preview_warm_size == Some(size);
                if key == self.preview_cache_key
                    && is_current_size
                    && self.preview_protocol.is_none()
                {
                    // The deferred (scroll) path is waiting on this encode — promote it
                    // to the live preview now that it's ready, without blocking the UI.
                    self.preview_protocol = Some(*protocol);
                    self.preview_error = None;
                    self.dispatched_generation = self.generation;
                } else if key != self.preview_cache_key || is_current_size {
                    self.remember_protocol(key, *protocol);
                }
            }
            ProtocolWarmOutcome::Failed { key, size } => {
                self.remove_protocol_inflight(key, size);
                if key == self.preview_cache_key
                    && self.preview_warm_size == Some(size)
                    && self.preview_protocol.is_none()
                {
                    self.dispatched_generation = self.generation.saturating_sub(1);
                }
            }
            ProtocolWarmOutcome::Skipped(jobs) => {
                for (key, size) in jobs {
                    self.remove_protocol_inflight(key, size);
                    if key == self.preview_cache_key
                        && self.preview_warm_size == Some(size)
                        && self.preview_protocol.is_none()
                    {
                        self.dispatched_generation = self.generation.saturating_sub(1);
                    }
                }
            }
        }
    }

    fn handle_warm_result(&mut self, result: WarmResult) {
        match result.outcome {
            WarmOutcome::Ready { latex, px, image } => {
                let key = render_cache::key(&latex, px);
                self.warm_inflight.remove(&key);
                match image {
                    Ok(raw) => {
                        let display = self.graphics.recolor(raw);
                        // Encode off-thread — the active preview at priority, neighbours as
                        // background work — so landing on a neighbour finds its protocol
                        // already cached instead of re-rendering. The pixel scan for vertical
                        // centering runs in the worker, never on the UI thread.
                        let priority =
                            key == self.preview_cache_key && self.preview_protocol.is_none();
                        self.queue_protocol_warm_inner(key, display.clone(), priority);
                        self.remember_cache(key, display);
                    }
                    Err(_) => {
                        self.warm_failed.insert(key);
                    }
                }
            }
            WarmOutcome::Skipped(jobs) => {
                for job in jobs {
                    self.warm_inflight
                        .remove(&render_cache::key(&job.latex, job.px));
                }
            }
        }
    }

    fn handle_render_result(&mut self, result: RenderResult) {
        let key = render_cache::key(&result.latex, result.px);
        if self.render_inflight_key == Some(key) {
            self.render_inflight_key = None;
        }
        let is_current_generation = result.generation >= self.generation;
        let is_current_preview = key == self.preview_cache_key;
        match result.image {
            Ok(raw) => {
                let display = self.graphics.recolor(raw);
                if !is_current_generation && !is_current_preview {
                    // Stale render (a neighbour that scrolled past) — still encode it
                    // off-thread so revisiting it is instant.
                    self.queue_protocol_warm_inner(key, display.clone(), false);
                    self.remember_cache(key, display);
                    return;
                }

                self.preview_error = None;
                // Stash whatever was being shown while this rendered (may be a
                // different equation that stayed visible during the debounce window).
                if self.preview_cache_key != key {
                    if let Some(old_proto) = self.preview_protocol.take() {
                        self.remember_protocol(self.preview_cache_key, old_proto);
                    }
                    self.preview_cache_key = key;
                }
                if self.preview_protocol.is_none() {
                    if let Some(protocol) = self.take_protocol(key) {
                        self.preview_protocol = Some(protocol);
                    } else {
                        // Encode off-thread; promoted into the preview when ready.
                        self.queue_current_protocol_warm(key, display.clone());
                    }
                }
                self.dispatched_generation = self.generation;
                self.remember_cache(key, display);
            }
            Err(err) => {
                if !is_current_generation && !is_current_preview {
                    return;
                }

                self.preview_error = Some(err);
                if !self.preview_preserve_on_error {
                    self.preview_protocol = None;
                }
                self.dispatched_generation = self.generation;
            }
        }
    }

    fn schedule_selected(&mut self) {
        self.schedule_selected_inner(true);
    }

    fn schedule_selected_deferred(&mut self) {
        self.schedule_selected_inner(false);
    }

    /// `immediate == true` means a deliberate single selection (open editor, zoom,
    /// picker move) where a touch of synchronous work — a disk decode or a one-off
    /// encode — is acceptable to show the preview instantly. `immediate == false`
    /// is the rapid-scroll path: it must never block the UI thread, so any missing
    /// encode is handed to the background warmers and a spinner is shown until ready.
    fn schedule_selected_inner(&mut self, immediate: bool) {
        let in_editor = matches!(
            self.mode,
            Mode::Editor
                | Mode::RelatedPicker
                | Mode::ConfirmRemoveRelated(_)
                | Mode::ReferenceEditor
                | Mode::ConfirmRemoveReference(_)
        );
        let latex = if in_editor {
            self.editor
                .as_ref()
                .map(|editor| editor.field_text(2))
                .unwrap_or_default()
        } else {
            self.items
                .get(self.cursor)
                .map(|item| item.latex.clone())
                .unwrap_or_default()
        };
        let px = if in_editor {
            self.selected.as_ref().map(|eq| eq.px_height).unwrap_or(48)
        } else {
            self.items
                .get(self.cursor)
                .map(|item| item.px_height)
                .unwrap_or(48)
        };
        self.schedule_latex_inner(latex, px, immediate);
        if !in_editor {
            self.schedule_warm_neighbors();
        }
    }

    fn schedule_latex(&mut self, latex: String, px: u32) {
        self.schedule_latex_inner(latex, px, true);
    }

    fn effective_render_px(&self, preferred_px: u32) -> u32 {
        effective_render_px(preferred_px, self.preview_warm_size, self.cell_size_px)
    }

    fn default_equation_px(&self) -> u32 {
        default_equation_px(self.cell_size_px)
    }

    fn schedule_latex_inner(&mut self, latex: String, px: u32, immediate: bool) {
        self.preview_latex = latex;
        self.preview_px = px;
        self.preview_render_px = self.effective_render_px(px);
        self.preview_preserve_on_error = matches!(
            self.mode,
            Mode::Editor
                | Mode::RelatedPicker
                | Mode::ConfirmRemoveRelated(_)
                | Mode::ReferenceEditor
                | Mode::ConfirmRemoveReference(_)
        );
        self.generation = self.generation.saturating_add(1);
        self.last_change = Instant::now();
        let new_key = render_cache::key(&self.preview_latex, self.preview_render_px);

        if new_key != self.preview_cache_key {
            // Switching to a different equation: stash the current encoded protocol so
            // coming back to it doesn't require re-encoding.
            if let Some(proto) = self.preview_protocol.take() {
                self.remember_protocol(self.preview_cache_key, proto);
            }
            self.preview_cache_key = new_key;
            self.preview_error = None;
        }

        if self.preview_protocol.is_some() {
            // Already displaying the right equation — nothing to do.
            self.dispatched_generation = self.generation;
            self.render_inflight_key = None;
            return;
        }

        // An already-encoded protocol can be shown without any work on the UI thread.
        if let Some(protocol) = self.take_protocol(new_key) {
            self.preview_protocol = Some(protocol);
            self.preview_error = None;
            self.dispatched_generation = self.generation;
            self.render_inflight_key = None;
            return;
        }

        if !immediate {
            return;
        }

        // The decoded image may be cached even when no encoded protocol exists yet.
        // Encode it off-thread for deliberate selections; the rapid-scroll path leaves
        // this for the debounce render so navigation never synchronously scans pixels.
        if let Some(display) = self.cache.get(&new_key).cloned() {
            if self.queue_current_protocol_warm(new_key, display) {
                self.preview_error = None;
                self.dispatched_generation = self.generation;
                self.render_inflight_key = None;
            }
            // If the encode could not be queued (e.g. no preview size yet) we fall through
            // leaving generation != dispatched_generation so the debounced full render
            // picks it up once scrolling settles.
            return;
        }

        // Single selection: a one-off disk decode is acceptable to get the image into the
        // cache promptly. The encode still happens off-thread.
        if let Some(raw) = render_cache::load(&self.preview_latex, self.preview_render_px) {
            let display = self.graphics.recolor(raw);
            self.remember_cache(new_key, display.clone());
            if self.queue_current_protocol_warm(new_key, display) {
                self.preview_error = None;
                self.dispatched_generation = self.generation;
                self.render_inflight_key = None;
            }
            // If no preview size is known yet, set_preview_warm_size kicks the encode once
            // the pane is first drawn; until then the debounced full render is the fallback.
        }
    }

    fn schedule_warm_neighbors(&mut self) {
        if self.items.is_empty() {
            return;
        }

        let mut jobs = Vec::new();
        let mut seen = HashSet::new();
        for distance in 1..=WARM_RADIUS {
            for index in [
                self.cursor.checked_sub(distance),
                self.cursor.checked_add(distance),
            ]
            .into_iter()
            .flatten()
            {
                let Some(item) = self.items.get(index) else {
                    continue;
                };
                let item_px = self.effective_render_px(item.px_height);
                let key = render_cache::key(&item.latex, item_px);
                if !seen.insert(key)
                    || self.warm_inflight.contains(&key)
                    || self.warm_failed.contains(&key)
                {
                    continue;
                }

                // Cheap membership checks before any image clone: a neighbour that is
                // already encoded or in-flight needs no work.
                if self.is_protocol_warm(key) {
                    continue;
                }

                // Decoded image cached but not yet encoded — encode it off-thread so the
                // protocol is ready the moment the cursor lands on this neighbour. The
                // pixel scan for vertical centering happens in the worker, not here.
                if let Some(display) = self.cache.get(&key).cloned() {
                    self.queue_protocol_warm_inner(key, display, false);
                    continue;
                }

                // Not rendered yet — render it off-thread.
                jobs.push(WarmJob {
                    latex: item.latex.clone(),
                    px: item_px,
                });
                self.warm_inflight.insert(key);
            }
        }

        if !jobs.is_empty() {
            self.warm_worker.send(jobs);
        }
    }

    /// Ensure an encoded protocol for `key` is, or will become, available. Returns
    /// `true` when the protocol is already cached/in-flight or a new encode was queued,
    /// and `false` when no encode could be arranged (no known preview size yet).
    fn queue_current_protocol_warm(&mut self, key: u64, display: RgbaImage) -> bool {
        self.queue_protocol_warm_inner(key, display, true)
    }

    fn queue_protocol_warm_inner(&mut self, key: u64, display: RgbaImage, priority: bool) -> bool {
        if self.is_protocol_warm(key) {
            return true;
        }
        let Some(available) = self.preview_warm_size else {
            return false;
        };

        self.protocol_warm_inflight.insert(key, available);
        self.protocol_warm_worker.send(vec![ProtocolWarmJob {
            key,
            source: ProtocolWarmSource::Image {
                display,
                graphics: self.graphics.clone(),
            },
            size: available,
            priority,
        }]);
        true
    }

    /// Whether an encoded protocol for `key` is already cached, being encoded, or
    /// currently on screen — i.e. nothing needs to be (re-)queued for it.
    fn is_protocol_warm(&self, key: u64) -> bool {
        self.protocol_cache.contains_key(&key)
            || self
                .protocol_warm_inflight
                .get(&key)
                .is_some_and(|size| Some(*size) == self.preview_warm_size)
            || (key == self.preview_cache_key && self.preview_protocol.is_some())
    }

    /// The draw code calls this when the live `preview_protocol` is not yet encoded for
    /// the area it's about to be drawn into. Rather than let the widget encode on the UI
    /// thread (which blocks scrolling), we move the protocol to the background encoder and
    /// show a spinner; `tick_render` promotes the result back into the preview when ready.
    pub fn request_preview_encode(&mut self, available: Size) {
        if available.width == 0 || available.height == 0 {
            return;
        }
        let key = self.preview_cache_key;
        if self.protocol_warm_inflight.get(&key) == Some(&available) {
            // Another encode for this equation is already in flight; keep the stale protocol
            // visible until that result is promoted.
            return;
        }
        let Some(protocol) = self.preview_protocol.take() else {
            return;
        };
        let source = if let Some(display) = self.cache.get(&key).cloned() {
            ProtocolWarmSource::Image {
                display,
                graphics: self.graphics.clone(),
            }
        } else {
            ProtocolWarmSource::Protocol(protocol)
        };
        self.protocol_warm_inflight.insert(key, available);
        self.protocol_warm_worker.send(vec![ProtocolWarmJob {
            key,
            source,
            size: available,
            priority: true,
        }]);
    }

    fn open_editor(&mut self, id: Option<EquationId>) {
        let equation = id.and_then(|eq_id| self.store.get(eq_id).ok());
        self.selected = equation.clone();
        let initial_related = equation
            .as_ref()
            .map(|eq| eq.related.clone())
            .unwrap_or_default();
        let initial_references = equation
            .as_ref()
            .map(|eq| eq.references.clone())
            .unwrap_or_default();
        let field_values = if let Some(eq) = equation {
            [
                eq.name,
                eq.description,
                eq.latex,
                String::new(),
                eq.tags.join(", "),
                format_variables(&eq.variables),
                format_related(&initial_related, &self.all_items),
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
        let fields = field_values
            .each_ref()
            .map(|value| textarea_from_text(value));
        self.editor = Some(EditorState {
            editing: id,
            last_saved_signature: fields_signature(
                &field_values,
                &initial_related,
                &initial_references,
            ),
            fields,
            focus: 0,
            related_cursor: 0,
            reference_cursor: 0,
            references: initial_references,
            reference_form: ReferenceForm::empty(),
            dirty: false,
            last_change: Instant::now(),
            related_picker: RelatedPickerState {
                cursor: 0,
                list_scroll_offset: 0,
                list_visible_height: 0,
                selected: Vec::new(),
                query: String::new(),
                query_cursor: 0,
                focus: RelatedPickerFocus::Search,
            },
            related: initial_related,
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
        match key.code {
            KeyCode::Char('r') if focused == 6 => {
                self.open_related_picker();
                return;
            }
            KeyCode::Char('a') if focused == 3 => {
                self.open_reference_form(None);
                return;
            }
            KeyCode::Char('k') if focused == 3 => {
                editor.reference_cursor = editor.reference_cursor.saturating_sub(1);
                return;
            }
            KeyCode::Char('j') if focused == 3 => {
                let max = editor.references.len().saturating_sub(1);
                editor.reference_cursor = (editor.reference_cursor + 1).min(max);
                return;
            }
            KeyCode::Char('d') if focused == 3 => {
                if let Some(idx) = self.current_reference_index() {
                    self.mode = Mode::ConfirmRemoveReference(idx);
                }
                return;
            }
            KeyCode::Char('k') if focused == 6 => {
                editor.related_cursor = editor.related_cursor.saturating_sub(1);
                return;
            }
            KeyCode::Char('j') if focused == 6 => {
                let max = editor.related.len().saturating_sub(1);
                editor.related_cursor = (editor.related_cursor + 1).min(max);
                return;
            }
            KeyCode::Char('d') if focused == 6 => {
                if let Some(id) = self.current_related_id() {
                    self.mode = Mode::ConfirmRemoveRelated(id);
                }
                return;
            }
            KeyCode::Enter if matches!(focused, 0 | 4) => return,
            KeyCode::Enter if focused == 3 => {
                if let Some(idx) = self.current_reference_index() {
                    self.open_reference_form(Some(idx));
                }
                return;
            }
            KeyCode::Enter if focused == 6 => {
                self.open_selected_related_detail();
                return;
            }
            _ if focused != 6 && focused != 3 && editor.fields[focused].input(key) => {}
            _ => return,
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
        related_picker_items_for(&self.all_items, editor.editing)
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
        editor.related_picker.selected = editor.related.clone();
        editor.related_picker.query.clear();
        editor.related_picker.query_cursor = 0;
        editor.related_picker.focus = RelatedPickerFocus::Search;
        let items = related_picker_items_for(&self.all_items, editor.editing);
        if editor.related_picker.cursor >= items.len() {
            editor.related_picker.cursor = items.len().saturating_sub(1);
        }
        editor.related_picker.list_scroll_offset = editor
            .related_picker
            .list_scroll_offset
            .min(editor.related_picker.cursor);
        self.mode = Mode::RelatedPicker;
        self.schedule_related_picker_preview();
    }

    fn current_reference_index(&self) -> Option<usize> {
        let editor = self.editor.as_ref()?;
        if editor.focus != 3 || editor.references.is_empty() {
            return None;
        }
        Some(editor.reference_cursor.min(editor.references.len() - 1))
    }

    fn open_reference_form(&mut self, index: Option<usize>) {
        let form = {
            let Some(editor) = self.editor.as_ref() else {
                return;
            };
            match index {
                Some(i) if i < editor.references.len() => {
                    ReferenceForm::from_reference(&editor.references[i], i)
                }
                _ => ReferenceForm::empty(),
            }
        };
        if let Some(editor) = self.editor.as_mut() {
            editor.reference_form = form;
        }
        self.mode = Mode::ReferenceEditor;
    }

    fn input_reference_form(&mut self, key: crossterm::event::KeyEvent) {
        if let Some(editor) = &mut self.editor {
            let focus = editor.reference_form.focus;
            editor.reference_form.fields[focus].input(key);
            editor.reference_form.error = None;
        }
    }

    fn reference_form_next_field(&mut self) {
        if let Some(editor) = &mut self.editor {
            let n = editor.reference_form.fields.len();
            editor.reference_form.focus = (editor.reference_form.focus + 1) % n;
        }
    }

    fn reference_form_prev_field(&mut self) {
        if let Some(editor) = &mut self.editor {
            let n = editor.reference_form.fields.len();
            editor.reference_form.focus = (editor.reference_form.focus + n - 1) % n;
        }
    }

    fn save_reference_form(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let authors = editor.reference_form.field_text(0).trim().to_string();
        let year_raw = editor.reference_form.field_text(1).trim().to_string();
        let title = editor.reference_form.field_text(2).trim().to_string();
        let doi_raw = editor.reference_form.field_text(3).trim().to_string();
        let url_raw = editor.reference_form.field_text(4).trim().to_string();

        if title.is_empty() && authors.is_empty() {
            editor.reference_form.error = Some("Enter at least a title or authors".to_string());
            return;
        }
        let year = if year_raw.is_empty() {
            None
        } else {
            match year_raw.parse::<i32>() {
                Ok(y) => Some(y),
                Err(_) => {
                    editor.reference_form.error = Some("Year must be a number".to_string());
                    return;
                }
            }
        };

        let (doi, url) = if !doi_raw.is_empty() {
            let doi = normalize_doi(&doi_raw).unwrap_or(doi_raw);
            let url = (!url_raw.is_empty()).then_some(url_raw);
            (Some(doi), url)
        } else if let Some(doi) = normalize_doi(&url_raw) {
            (Some(doi), None)
        } else {
            (None, (!url_raw.is_empty()).then_some(url_raw))
        };

        let reference = Reference {
            authors,
            year,
            title,
            doi,
            url,
        };
        let editing = editor.reference_form.editing;
        match editing {
            Some(i) if i < editor.references.len() => editor.references[i] = reference,
            _ => editor.references.push(reference),
        }
        editor.reference_cursor = match editing {
            Some(i) => i.min(editor.references.len().saturating_sub(1)),
            None => editor.references.len().saturating_sub(1),
        };
        mark_editor_dirty(editor);
        self.mode = Mode::Editor;
    }

    fn remove_reference(&mut self, index: usize) {
        if let Some(editor) = &mut self.editor {
            if index < editor.references.len() {
                editor.references.remove(index);
                editor.reference_cursor = editor
                    .reference_cursor
                    .min(editor.references.len().saturating_sub(1));
                mark_editor_dirty(editor);
            }
        }
    }

    fn schedule_related_picker_preview(&mut self) {
        let Some((latex, px)) = self
            .related_picker_preview_item()
            .map(|item| (item.latex.clone(), RELATED_PICKER_PREVIEW_PX))
        else {
            return;
        };
        self.schedule_latex(latex, px);
    }

    fn related_picker_preview_item(&self) -> Option<&EquationSummary> {
        let editor = self.editor.as_ref()?;
        related_picker_items_for(&self.all_items, editor.editing)
            .into_iter()
            .filter(|item| fuzzy_matches_item(&editor.related_picker.query, item))
            .nth(editor.related_picker.cursor)
    }

    fn toggle_related_picker_focus(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.related_picker.focus = match editor.related_picker.focus {
            RelatedPickerFocus::Search => RelatedPickerFocus::List,
            RelatedPickerFocus::List => RelatedPickerFocus::Search,
        };
    }

    fn move_related_picker_cursor(&mut self, down: bool) {
        if !self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.related_picker.focus == RelatedPickerFocus::List)
        {
            return;
        }
        let max = self.filtered_related_picker_items().len().saturating_sub(1);
        if let Some(editor) = &mut self.editor {
            if down {
                editor.related_picker.cursor = (editor.related_picker.cursor + 1).min(max);
                let visible =
                    list_visible_item_count(editor.related_picker.list_visible_height).max(1);
                if editor.related_picker.cursor
                    >= editor.related_picker.list_scroll_offset + visible
                {
                    editor.related_picker.list_scroll_offset =
                        editor.related_picker.cursor + 1 - visible;
                }
            } else {
                editor.related_picker.cursor = editor.related_picker.cursor.saturating_sub(1);
                if editor.related_picker.cursor < editor.related_picker.list_scroll_offset {
                    editor.related_picker.list_scroll_offset = editor.related_picker.cursor;
                }
            }
        }
        self.schedule_related_picker_preview();
    }

    fn related_picker_space_or_toggle(&mut self) {
        match self
            .editor
            .as_ref()
            .map(|editor| editor.related_picker.focus)
        {
            Some(RelatedPickerFocus::Search) => self.insert_related_picker_query_char(' '),
            Some(RelatedPickerFocus::List) => self.toggle_related_picker_selection(),
            None => {}
        }
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
        if self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.related_picker.focus == RelatedPickerFocus::List)
        {
            match key.code {
                KeyCode::Char('j') => self.move_related_picker_cursor(true),
                KeyCode::Char('k') => self.move_related_picker_cursor(false),
                _ => {}
            }
            return;
        }
        let Some(editor) = &mut self.editor else {
            return;
        };
        let mut changed = false;
        match key.code {
            KeyCode::Char(ch) => {
                editor
                    .related_picker
                    .query
                    .insert(editor.related_picker.query_cursor, ch);
                editor.related_picker.query_cursor += ch.len_utf8();
                changed = true;
            }
            KeyCode::Backspace => {
                if editor.related_picker.query_cursor > 0 {
                    let previous = prev_boundary(
                        &editor.related_picker.query,
                        editor.related_picker.query_cursor,
                    );
                    editor
                        .related_picker
                        .query
                        .drain(previous..editor.related_picker.query_cursor);
                    editor.related_picker.query_cursor = previous;
                    changed = true;
                }
            }
            KeyCode::Delete => {
                if editor.related_picker.query_cursor < editor.related_picker.query.len() {
                    let next = next_boundary(
                        &editor.related_picker.query,
                        editor.related_picker.query_cursor,
                    );
                    editor
                        .related_picker
                        .query
                        .drain(editor.related_picker.query_cursor..next);
                    changed = true;
                }
            }
            KeyCode::Left => {
                editor.related_picker.query_cursor = prev_boundary(
                    &editor.related_picker.query,
                    editor.related_picker.query_cursor,
                );
            }
            KeyCode::Right => {
                editor.related_picker.query_cursor = next_boundary(
                    &editor.related_picker.query,
                    editor.related_picker.query_cursor,
                );
            }
            KeyCode::Home => editor.related_picker.query_cursor = 0,
            KeyCode::End => editor.related_picker.query_cursor = editor.related_picker.query.len(),
            _ => {}
        }
        if !changed {
            return;
        }
        editor.related_picker.cursor = 0;
        editor.related_picker.list_scroll_offset = 0;
        self.schedule_related_picker_preview();
    }

    fn insert_related_picker_query_char(&mut self, ch: char) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor
            .related_picker
            .query
            .insert(editor.related_picker.query_cursor, ch);
        editor.related_picker.query_cursor += ch.len_utf8();
        editor.related_picker.cursor = 0;
        editor.related_picker.list_scroll_offset = 0;
        self.schedule_related_picker_preview();
    }

    fn apply_related_picker(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.related = editor.related_picker.selected.clone();
        let display = format_related(&editor.related, &self.all_items);
        editor.set_field_text(6, display);
        editor.related_cursor = editor
            .related_cursor
            .min(editor.related.len().saturating_sub(1));
        mark_editor_dirty(editor);
        self.mode = Mode::Editor;
        self.schedule_selected();
    }

    fn open_selected_related_detail(&mut self) {
        let (focus, id_opt) = match &self.editor {
            Some(editor) => (
                editor.focus,
                editor.related.get(editor.related_cursor).copied(),
            ),
            None => return,
        };
        if focus != 6 {
            return;
        }
        let Some(id) = id_opt else {
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
        editor.related.get(editor.related_cursor).copied()
    }

    fn remove_related_from_editor(&mut self, id: EquationId) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.related.retain(|related_id| *related_id != id);
        let display = format_related(&editor.related, &self.all_items);
        editor.set_field_text(6, display);
        editor.related_cursor = editor
            .related_cursor
            .min(editor.related.len().saturating_sub(1));
        mark_editor_dirty(editor);
    }

    fn save_editor(&mut self) -> anyhow::Result<()> {
        self.persist_editor(false)
    }

    fn copy_current_equation(&mut self) -> anyhow::Result<()> {
        let Some(source_id) = self.selected_id() else {
            return Ok(());
        };
        let source = self.store.get(source_id)?;
        let mut clone = Equation::new(format!("[clone] {}", source.name), source.latex.clone());
        clone.description = source.description;
        clone.references = source.references;
        clone.tags = source.tags;
        clone.variables = source.variables;
        clone.related = source.related;
        clone.px_height = source.px_height;
        let clone_id = clone.id;

        self.store.insert_allowing_duplicate_latex(&clone)?;
        self.reload()?;
        if let Some(index) = self.items.iter().position(|item| item.id == clone_id) {
            self.cursor = index;
        }
        self.selected = self.store.get(clone_id).ok();
        self.mode = Mode::Browser;
        self.status = "Equation copied".to_string();
        self.notification = Some(Notification {
            message: "equation copied".to_string(),
            created_at: Instant::now(),
        });
        self.schedule_selected();
        Ok(())
    }

    fn copy_selected_latex_to_clipboard(&mut self) -> anyhow::Result<()> {
        let Some(latex) = self
            .selected
            .as_ref()
            .map(|equation| equation.latex.clone())
        else {
            self.status = "No equation selected".to_string();
            return Ok(());
        };
        crate::clipboard::copy_text(&latex)?;
        self.status = "LaTeX copied to clipboard".to_string();
        self.notification = Some(Notification {
            message: "latex copied".to_string(),
            created_at: Instant::now(),
        });
        Ok(())
    }

    fn persist_editor(&mut self, exit_after_save: bool) -> anyhow::Result<()> {
        let Some(editor) = &self.editor else {
            return Ok(());
        };
        let fields = editor.field_texts();
        let editing = editor.editing;
        let last_saved_signature = editor.last_saved_signature.clone();
        let related_ids = editor.related.clone();
        let references = editor.references.clone();
        if fields[0].trim().is_empty() {
            return Ok(());
        }
        if fields[2].trim().is_empty() {
            return Ok(());
        }
        let signature = fields_signature(&fields, &related_ids, &references);
        if signature == last_saved_signature {
            if let Some(editor) = &mut self.editor {
                editor.dirty = false;
            }
            return Ok(());
        }
        validate_latex(&fields[2]).map_err(anyhow::Error::msg)?;
        let mut equation = if let Some(id) = editing {
            self.store.get(id)?
        } else {
            let mut equation =
                Equation::new(fields[0].trim().to_string(), fields[2].trim().to_string());
            equation.px_height = self.default_equation_px();
            equation
        };
        equation.name = fields[0].trim().to_string();
        equation.description = fields[1].trim().to_string();
        equation.latex = fields[2].trim().to_string();
        equation.references = references.clone();
        equation.tags = {
            let mut seen = std::collections::HashSet::new();
            fields[4]
                .split(',')
                .map(str::trim)
                .filter(|tag| !tag.is_empty())
                .filter(|tag| seen.insert(*tag))
                .map(ToOwned::to_owned)
                .collect()
        };
        equation.variables = parse_variables(&fields[5]);
        equation.related = related_ids;
        equation.updated_at = nullspace_core::store::now_rfc3339();
        let saved_id = equation.id;
        let save_result = if editing.is_some() {
            self.store.update(&equation)
        } else {
            self.store.insert(&equation)
        };
        match save_result {
            Ok(()) => {}
            Err(nullspace_core::Error::Duplicate(_)) => {
                if let Some(editor) = &mut self.editor {
                    editor.dirty = false;
                    editor.last_saved_signature = signature;
                }
                self.status = "An equation with this LaTeX already exists.".to_string();
                return Ok(());
            }
            Err(err) => return Err(err.into()),
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
        while self.cache_order.len() > IMAGE_CACHE_CAPACITY {
            if let Some(old) = self.cache_order.pop_front() {
                self.cache.remove(&old);
            }
        }
    }

    fn remember_protocol(&mut self, key: u64, protocol: StatefulProtocol) {
        self.protocol_cache_order
            .retain(|cached_key| *cached_key != key);
        self.protocol_cache_order.push_back(key);
        self.protocol_cache.insert(key, protocol);
        while self.protocol_cache_order.len() > PROTOCOL_CACHE_CAPACITY {
            if let Some(old) = self.protocol_cache_order.pop_front() {
                self.protocol_cache.remove(&old);
            }
        }
    }

    fn take_protocol(&mut self, key: u64) -> Option<StatefulProtocol> {
        self.protocol_cache_order
            .retain(|cached_key| *cached_key != key);
        self.protocol_cache.remove(&key)
    }

    fn remove_protocol_inflight(&mut self, key: u64, size: Size) {
        if self.protocol_warm_inflight.get(&key) == Some(&size) {
            self.protocol_warm_inflight.remove(&key);
        }
    }

    fn adjust_zoom(&mut self, increase: bool) -> anyhow::Result<()> {
        let Some((id, current_px)) = self
            .items
            .get(self.cursor)
            .map(|item| (item.id, item.px_height))
        else {
            return Ok(());
        };
        let new_px = if increase {
            (current_px + 16).min(512)
        } else {
            current_px.saturating_sub(16).max(16)
        };
        if new_px == current_px {
            return Ok(());
        }
        self.store.update_px_height(id, new_px)?;
        if let Some(selected) = &mut self.selected {
            if selected.id == id {
                selected.px_height = new_px;
            }
        }
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.px_height = new_px;
        }
        if let Some(item) = self.all_items.iter_mut().find(|i| i.id == id) {
            item.px_height = new_px;
        }
        self.schedule_selected();
        Ok(())
    }
}

fn seed(store: &mut Store, default_equation_px: u32) -> anyhow::Result<()> {
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
        eq.px_height = default_equation_px;
        store.insert(&eq)?;
    }
    Ok(())
}

fn textarea_from_text(text: &str) -> TextArea<'static> {
    let lines = textarea_lines(text);
    let cursor = textarea_end_cursor(&lines);
    let mut textarea = TextArea::new(lines.clone());
    textarea.set_lines(lines, cursor);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    textarea
}

fn set_textarea_text(textarea: &mut TextArea<'static>, text: String) {
    let lines = textarea_lines(&text);
    let cursor = textarea_end_cursor(&lines);
    textarea.set_lines(lines, cursor);
}

fn textarea_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        vec![String::new()]
    } else {
        text.split('\n').map(ToOwned::to_owned).collect()
    }
}

fn textarea_end_cursor(lines: &[String]) -> (usize, usize) {
    let row = lines.len().saturating_sub(1);
    let column = lines.last().map(|line| line.chars().count()).unwrap_or(0);
    (row, column)
}

fn textarea_text(textarea: &TextArea<'_>) -> String {
    textarea.lines().join("\n")
}

fn format_variables(variables: &[Variable]) -> String {
    variables
        .iter()
        .map(|variable| format!("{} = {}", variable.symbol, variable.description))
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_variables(raw: &str) -> Vec<Variable> {
    let mut seen = std::collections::HashSet::new();
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
        .filter(|v| seen.insert(v.symbol.clone()))
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

fn fields_signature(
    fields: &[String; 7],
    related: &[EquationId],
    references: &[Reference],
) -> String {
    let related_part = related
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let references_part = references
        .iter()
        .map(|r| {
            format!(
                "{}|{}|{}|{}|{}",
                r.authors,
                r.year.map(|y| y.to_string()).unwrap_or_default(),
                r.title,
                r.doi.clone().unwrap_or_default(),
                r.url.clone().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    format!(
        "{}\u{1f}{}\u{1f}{}",
        fields.join("\u{1f}"),
        related_part,
        references_part
    )
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
    let needle = query.to_lowercase();
    item.name.to_lowercase().contains(&needle) || item.latex.to_lowercase().contains(&needle)
}

fn effective_render_px(
    preferred_px: u32,
    preview_size: Option<Size>,
    cell_size_px: TerminalCellSize,
) -> u32 {
    let preferred_px = preferred_px.clamp(MIN_RENDER_PX, MAX_RENDER_PX);
    let Some(size) = preview_size else {
        return preferred_px;
    };
    let cell_height = terminal_cell_height_px(cell_size_px);
    let pane_px = u32::from(size.height)
        .saturating_mul(cell_height)
        .saturating_sub(PREVIEW_RENDER_EDGE_GUARD_PX)
        .clamp(MIN_RENDER_PX, MAX_RENDER_PX);
    preferred_px.min(pane_px)
}

fn default_equation_px(cell_size_px: TerminalCellSize) -> u32 {
    terminal_cell_height_px(cell_size_px)
        .saturating_mul(DEFAULT_EQUATION_ROWS)
        .clamp(MIN_RENDER_PX, MAX_RENDER_PX)
}

fn terminal_cell_height_px(cell_size_px: TerminalCellSize) -> u32 {
    match u32::from(cell_size_px.height) {
        0 => FALLBACK_TERMINAL_CELL_PX_HEIGHT,
        height => height,
    }
}

fn warm_result_key(result: &WarmResult) -> Option<u64> {
    match &result.outcome {
        WarmOutcome::Ready { latex, px, .. } => Some(render_cache::key(latex, *px)),
        WarmOutcome::Skipped(_) => None,
    }
}

fn protocol_result_key(result: &ProtocolWarmResult) -> Option<u64> {
    match &result.outcome {
        ProtocolWarmOutcome::Ready { key, .. } | ProtocolWarmOutcome::Failed { key, .. } => {
            Some(*key)
        }
        ProtocolWarmOutcome::Skipped(_) => None,
    }
}

fn result_budget_spent(started: Instant) -> bool {
    started.elapsed() >= RESULT_TICK_BUDGET
}

// Each list item renders as 2 lines; spacers between items are 1 line each.
// From height H rows: fit k items where 3k-1 <= H, so k = (H+1)/3.
fn list_visible_item_count(height: u16) -> usize {
    (height as usize + 1) / 3
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

#[cfg(test)]
mod tests {
    use super::{
        default_equation_px, effective_render_px, fuzzy_matches_item, textarea_from_text,
        textarea_lines, textarea_text,
    };
    use crate::graphics::TerminalCellSize;
    use nullspace_core::{EquationId, EquationSummary};
    use ratatui::layout::Size;

    #[test]
    fn textarea_round_trips_multiline_text() {
        let text = "a\nb\nc";
        let textarea = textarea_from_text(text);
        assert_eq!(textarea_text(&textarea), text);
    }

    #[test]
    fn textarea_lines_preserve_trailing_empty_line() {
        assert_eq!(textarea_lines("a\n"), ["a".to_string(), String::new()]);
    }

    #[test]
    fn related_picker_search_matches_name_or_latex_only() {
        let description_only = EquationSummary {
            id: EquationId::new(),
            name: "BCS gap relation".to_string(),
            description: "Mentions Debye in prose".to_string(),
            latex: "\\Delta = 1.76 k_B T_c".to_string(),
            unicode_approx: "Δ = 1.76 k_B T_c".to_string(),
            px_height: 48,
        };
        let actual_match = EquationSummary {
            id: EquationId::new(),
            name: "Debye heat capacity".to_string(),
            description: "Low-temperature lattice heat capacity".to_string(),
            latex: "C_V = \\beta T^3".to_string(),
            unicode_approx: "C_V = β T³".to_string(),
            px_height: 48,
        };

        assert!(!fuzzy_matches_item("Debye", &description_only));
        assert!(fuzzy_matches_item("Debye", &actual_match));
    }

    #[test]
    fn effective_render_px_caps_to_preview_height() {
        let size = Some(Size {
            width: 80,
            height: 5,
        });
        let cell_size = TerminalCellSize { height: 20 };

        assert_eq!(effective_render_px(512, size, cell_size), 100);
    }

    #[test]
    fn effective_render_px_does_not_upscale_small_equations() {
        let size = Some(Size {
            width: 80,
            height: 20,
        });
        let cell_size = TerminalCellSize { height: 20 };

        assert_eq!(effective_render_px(48, size, cell_size), 48);
    }

    #[test]
    fn effective_render_px_uses_detected_cell_height() {
        let size = Some(Size {
            width: 80,
            height: 5,
        });
        let cell_size = TerminalCellSize { height: 18 };

        assert_eq!(effective_render_px(512, size, cell_size), 90);
    }

    #[test]
    fn effective_render_px_uses_full_detected_cell_box() {
        let size = Some(Size {
            width: 80,
            height: 5,
        });
        let cell_size = TerminalCellSize { height: 26 };

        assert_eq!(effective_render_px(512, size, cell_size), 130);
    }

    #[test]
    fn default_equation_px_is_five_cell_heights() {
        let cell_size = TerminalCellSize { height: 20 };

        assert_eq!(default_equation_px(cell_size), 100);
    }
}
