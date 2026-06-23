use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::time::{Duration, Instant};

use image::RgbaImage;
use nullspace_core::{
    render::render_image, Equation, EquationId, EquationSummary, Reference, Store, Variable,
};
use ratatui::layout::Size;
use ratatui_image::protocol::StatefulProtocol;

use crate::action::Action;
use crate::graphics::Graphics;
use crate::protocol_warm_worker::{ProtocolWarmJob, ProtocolWarmOutcome, ProtocolWarmWorker};
use crate::render_cache;
use crate::render_worker::{RenderJob, RenderWorker};
use crate::warm_worker::{WarmJob, WarmOutcome, WarmWorker};

const PROTOCOL_CACHE_CAPACITY: usize = 16;
const WARM_RADIUS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Browser,
    Search,
    VariableLookup,
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

#[derive(Clone)]
pub enum BrowserFilter {
    None,
    Search(String),
    Variable(String),
}

pub struct AppState {
    pub store: Store,
    pub mode: Mode,
    pub all_items: Vec<EquationSummary>,
    pub items: Vec<EquationSummary>,
    pub browser_filter: BrowserFilter,
    pub browser_filter_cursor: usize,
    pub cursor: usize,
    pub focus: Pane,
    pub should_quit: bool,
    pub graphics_ok: bool,
    pub status: String,
    pub selected: Option<Equation>,
    pub editor: Option<EditorState>,
    pub preview: Option<RgbaImage>,
    pub preview_protocol: Option<StatefulProtocol>,
    pub preview_error: Option<String>,
    pub preview_latex: String,
    pub preview_px: u32,
    pub preview_preserve_on_error: bool,
    pub notification: Option<Notification>,
    editor_history: Vec<EditorState>,
    worker: RenderWorker,
    warm_worker: WarmWorker,
    protocol_warm_worker: ProtocolWarmWorker,
    generation: u64,
    dispatched_generation: u64,
    last_change: Instant,
    cache: HashMap<u64, RgbaImage>,
    cache_order: VecDeque<u64>,
    warm_inflight: HashSet<u64>,
    warm_failed: HashSet<u64>,
    protocol_warm_inflight: HashSet<u64>,
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
        if store.list()?.is_empty() {
            seed(&mut store)?;
        }
        let graphics_ok = graphics.graphics_ok;
        let mut app = Self {
            store,
            mode: Mode::Browser,
            all_items: Vec::new(),
            items: Vec::new(),
            browser_filter: BrowserFilter::None,
            browser_filter_cursor: 0,
            cursor: 0,
            focus: Pane::List,
            should_quit: false,
            graphics_ok,
            status: "Ready".to_string(),
            selected: None,
            editor: None,
            preview: None,
            preview_protocol: None,
            preview_error: None,
            preview_latex: String::new(),
            preview_px: 48,
            preview_preserve_on_error: false,
            notification: None,
            editor_history: Vec::new(),
            worker: RenderWorker::spawn(),
            warm_worker: WarmWorker::spawn(),
            protocol_warm_worker: ProtocolWarmWorker::spawn(),
            generation: 0,
            dispatched_generation: 0,
            last_change: Instant::now(),
            cache: HashMap::new(),
            cache_order: VecDeque::new(),
            warm_inflight: HashSet::new(),
            warm_failed: HashSet::new(),
            protocol_warm_inflight: HashSet::new(),
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
            BrowserFilter::Variable(symbol) => format!("Variable: {}", symbol),
        }
    }

    pub fn set_preview_warm_size(&mut self, size: Size) {
        if size.width == 0 || size.height == 0 || self.preview_warm_size == Some(size) {
            return;
        }
        self.preview_warm_size = Some(size);
        if matches!(
            self.mode,
            Mode::Browser | Mode::Search | Mode::VariableLookup
        ) {
            self.schedule_warm_neighbors();
        }
    }

    pub fn cache_status_for(&self, latex: &str, px: u32) -> CacheStatus {
        let key = render_cache::key(latex, px);
        if self.warm_inflight.contains(&key)
            || self.protocol_warm_inflight.contains(&key)
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

    fn refresh_items(&mut self) -> anyhow::Result<()> {
        self.items = match &self.browser_filter {
            BrowserFilter::None => self.all_items.clone(),
            BrowserFilter::Search(query) => self.store.search(query)?,
            BrowserFilter::Variable(symbol) => self.store.by_symbol(symbol)?,
        };
        self.cursor = self.cursor.min(self.items.len().saturating_sub(1));
        self.selected = self.selected_id().and_then(|id| self.store.get(id).ok());
        Ok(())
    }

    fn clear_browser_filter(&mut self) -> anyhow::Result<()> {
        self.browser_filter = BrowserFilter::None;
        self.browser_filter_cursor = 0;
        self.refresh_items()?;
        self.status = "Filter cleared".to_string();
        self.schedule_selected();
        Ok(())
    }

    fn input_browser_filter(&mut self, key: crossterm::event::KeyEvent) -> anyhow::Result<()> {
        use crossterm::event::KeyCode;
        let query = match &mut self.browser_filter {
            BrowserFilter::Search(query) | BrowserFilter::Variable(query) => query,
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
            let label = match &self.browser_filter {
                BrowserFilter::Search(_) => "Search",
                BrowserFilter::Variable(_) => "Variable lookup",
                BrowserFilter::None => "Filter",
            };
            self.status = format!("{label}: {} match(es)", self.items.len());
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
            Action::StartSearch => {
                self.browser_filter = BrowserFilter::Search(String::new());
                self.browser_filter_cursor = 0;
                self.mode = Mode::Search;
                self.refresh_items()?;
                self.status = "Search".to_string();
                self.schedule_selected();
                Ok(())
            }
            Action::StartVariableLookup => {
                self.browser_filter = BrowserFilter::Variable(String::new());
                self.browser_filter_cursor = 0;
                self.mode = Mode::VariableLookup;
                self.refresh_items()?;
                self.status = "Variable lookup".to_string();
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
                    Mode::Search | Mode::VariableLookup | Mode::Browser => Mode::Browser,
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
                        parse_related(&editor.fields[6], &self.all_items)
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

        while let Some(result) = self.protocol_warm_worker.try_recv() {
            self.protocol_warm_inflight.remove(&result.key);
            match result.outcome {
                ProtocolWarmOutcome::Ready(protocol) => {
                    if result.key != self.preview_cache_key || self.preview_protocol.is_none() {
                        self.remember_protocol(result.key, *protocol);
                    }
                }
                ProtocolWarmOutcome::Failed => {}
            }
        }

        while let Some(result) = self.warm_worker.try_recv() {
            let key = render_cache::key(&result.latex, result.px);
            self.warm_inflight.remove(&key);
            match result.outcome {
                WarmOutcome::Ready(Ok(raw)) => {
                    let display = self.graphics.recolor(raw);
                    self.remember_cache(key, display.clone());
                    self.queue_protocol_warm(key, display.clone());
                    if key == self.preview_cache_key && self.preview_protocol.is_none() {
                        let protocol = self
                            .take_protocol(key)
                            .unwrap_or_else(|| self.graphics.protocol_from(display.clone()));
                        self.preview_protocol = Some(protocol);
                        self.preview = Some(display);
                        self.preview_error = None;
                        self.dispatched_generation = self.generation;
                    }
                }
                WarmOutcome::Ready(Err(_)) => {
                    self.warm_failed.insert(key);
                }
                WarmOutcome::Skipped => {}
            }
        }

        while let Some(result) = self.worker.try_recv() {
            let key = render_cache::key(&result.latex, result.px);
            if self.render_inflight_key == Some(key) {
                self.render_inflight_key = None;
            }
            let is_current_generation = result.generation >= self.generation;
            let is_current_preview = key == self.preview_cache_key;
            match result.image {
                Ok(raw) => {
                    let display = self.graphics.recolor(raw);
                    self.remember_cache(key, display.clone());
                    if !is_current_generation && !is_current_preview {
                        continue;
                    }

                    self.preview_error = None;
                    // Stash whatever was being shown while this rendered (may be a
                    // different equation that stayed visible during the debounce window).
                    if let Some(old_proto) = self.preview_protocol.take() {
                        self.remember_protocol(self.preview_cache_key, old_proto);
                    }
                    self.preview_cache_key = key;
                    let protocol = self
                        .take_protocol(key)
                        .unwrap_or_else(|| self.graphics.protocol_from(display.clone()));
                    self.preview_protocol = Some(protocol);
                    self.preview = Some(display);
                    self.dispatched_generation = self.generation;
                }
                Err(err) => {
                    if !is_current_generation && !is_current_preview {
                        continue;
                    }

                    self.preview_error = Some(err);
                    if !self.preview_preserve_on_error {
                        self.preview = None;
                        self.preview_protocol = None;
                    }
                    self.dispatched_generation = self.generation;
                }
            }
        }

        if self.generation != self.dispatched_generation
            && self.last_change.elapsed() >= Duration::from_millis(150)
        {
            self.worker.send(RenderJob {
                generation: self.generation,
                latex: self.preview_latex.clone(),
                px: self.preview_px,
            });
            self.render_inflight_key =
                Some(render_cache::key(&self.preview_latex, self.preview_px));
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
        let in_editor = matches!(
            self.mode,
            Mode::Editor | Mode::RelatedPicker | Mode::ConfirmRemoveRelated(_)
        );
        let latex = if in_editor {
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
        let px = if in_editor {
            self.selected.as_ref().map(|eq| eq.px_height).unwrap_or(48)
        } else {
            self.items
                .get(self.cursor)
                .map(|item| item.px_height)
                .unwrap_or(48)
        };
        self.schedule_latex(latex, px);
        if !in_editor {
            self.schedule_warm_neighbors();
        }
    }

    fn schedule_latex(&mut self, latex: String, px: u32) {
        self.preview_latex = latex;
        self.preview_px = px;
        self.preview_preserve_on_error = matches!(
            self.mode,
            Mode::Editor | Mode::RelatedPicker | Mode::ConfirmRemoveRelated(_)
        );
        self.generation = self.generation.saturating_add(1);
        self.last_change = Instant::now();
        let new_key = render_cache::key(&self.preview_latex, self.preview_px);

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

        if let Some(display) = self.cache.get(&new_key).cloned() {
            let protocol = self
                .take_protocol(new_key)
                .unwrap_or_else(|| self.graphics.protocol_from(display.clone()));
            self.preview_protocol = Some(protocol);
            self.preview = Some(display);
            self.preview_error = None;
            self.dispatched_generation = self.generation;
            self.render_inflight_key = None;
            return;
        }

        if let Some(raw) = render_cache::load(&self.preview_latex, self.preview_px) {
            let display = self.graphics.recolor(raw);
            self.remember_cache(new_key, display.clone());
            let protocol = self
                .take_protocol(new_key)
                .unwrap_or_else(|| self.graphics.protocol_from(display.clone()));
            self.preview_protocol = Some(protocol);
            self.preview = Some(display);
            self.preview_error = None;
            self.dispatched_generation = self.generation;
            self.render_inflight_key = None;
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
                let key = render_cache::key(&item.latex, item.px_height);
                if !seen.insert(key) || self.warm_failed.contains(&key) {
                    continue;
                }

                if let Some(display) = self.cache.get(&key).cloned() {
                    self.queue_protocol_warm(key, display);
                    continue;
                }

                jobs.push(WarmJob {
                    latex: item.latex.clone(),
                    px: item.px_height,
                });
                self.warm_inflight.insert(key);
            }
        }

        if !jobs.is_empty() {
            self.warm_worker.send(jobs);
        }
    }

    fn queue_protocol_warm(&mut self, key: u64, display: RgbaImage) {
        if self.protocol_cache.contains_key(&key)
            || self.protocol_warm_inflight.contains(&key)
            || (key == self.preview_cache_key && self.preview_protocol.is_some())
        {
            return;
        }
        let Some(available) = self.preview_warm_size else {
            return;
        };

        let protocol = self.graphics.protocol_from(display);
        let size = protocol.size_for(ratatui_image::Resize::Fit(None), available);
        if size.width == 0 || size.height == 0 {
            return;
        }

        self.protocol_warm_inflight.insert(key);
        self.protocol_warm_worker.send(vec![ProtocolWarmJob {
            key,
            protocol,
            size,
        }]);
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
                format_related(&eq.related, &self.all_items),
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
                let max = parse_related(field, &self.all_items)
                    .len()
                    .saturating_sub(1);
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
        editor.related_picker.selected = parse_related(&editor.fields[6], &self.all_items);
        editor.related_picker.query.clear();
        let items = related_picker_items_for(&self.all_items, editor.editing);
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
        let max = related_picker_items_for(&self.all_items, editor.editing)
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
        editor.fields[6] = format_related(&editor.related_picker.selected, &self.all_items);
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
        let related = parse_related(&editor.fields[6], &self.all_items);
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
        parse_related(&editor.fields[6], &self.all_items)
            .get(editor.related_cursor)
            .copied()
    }

    fn remove_related_from_editor(&mut self, id: EquationId) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let mut related = parse_related(&editor.fields[6], &self.all_items);
        related.retain(|related_id| *related_id != id);
        editor.fields[6] = format_related(&related, &self.all_items);
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
        equation.related = parse_related(&editor.fields[6], &self.all_items);
        equation.updated_at = nullspace_core::store::now_rfc3339();
        let saved_id = equation.id;
        let save_result = if editor.editing.is_some() {
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
        while self.cache_order.len() > 64 {
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

    fn adjust_zoom(&mut self, increase: bool) -> anyhow::Result<()> {
        let Some(mut eq) = self.selected.clone() else {
            return Ok(());
        };
        let new_px = if increase {
            (eq.px_height + 16).min(512)
        } else {
            eq.px_height.saturating_sub(16).max(16)
        };
        if new_px == eq.px_height {
            return Ok(());
        }
        eq.px_height = new_px;
        self.store.update(&eq)?;
        let id = eq.id;
        self.selected = Some(eq);
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
