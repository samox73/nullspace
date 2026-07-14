use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use image::RgbaImage;
use nullspace_core::reference::{normalize_doi, normalize_pages, reference_link};
use nullspace_core::{
    Equation, EquationId, EquationSummary, Error, Quantity, QuantityId, Reference, Store,
    TrashEntry, Variable, render::validate_latex,
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
use crate::render_queue::{QueueJob, QueueResult, RenderQueue};

const IMAGE_CACHE_CAPACITY: usize = 128;
// Comfortably exceeds 2 * WARM_RADIUS so neighbours pre-encoded for both scroll
// directions (plus a little history) survive in the cache without thrashing.
const PROTOCOL_CACHE_CAPACITY: usize = 48;
const WARM_RADIUS: usize = 8;
const RELATED_PICKER_PREVIEW_PX: u32 = 512;
const RESULT_PULL_LIMIT: usize = 64;
const PROTOCOL_RESULTS_PER_TICK: usize = 16;
const QUEUE_RESULTS_PER_TICK: usize = 4;
const RESULT_TICK_BUDGET: Duration = Duration::from_millis(4);
const GRAPHICS_REFRESH_TIMEOUT: Duration = Duration::from_millis(180);
const MIN_RENDER_PX: u32 = 16;
const MAX_RENDER_PX: u32 = 512;
const FALLBACK_TERMINAL_CELL_PX_HEIGHT: u32 = 26;
const PREVIEW_RENDER_EDGE_GUARD_PX: u32 = 0;
const DEFAULT_EQUATION_ROWS: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Browser,
    Cmdline,
    Search,
    Editor,
    RelatedPicker,
    ReferenceEditor,
    VariableEditor,
    QuantityPicker,
    QuantityForm,
    QuantityResolver,
    Trash,
    TagPicker,
    ConfirmDelete(EquationId),
    ConfirmPurge(EquationId),
    ConfirmRemoveQuantity(QuantityId),
    ConfirmRemoveRelated(EquationId),
    ConfirmRemoveReference(usize),
    ConfirmRemoveVariable(usize),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorField {
    Name,
    Description,
    Latex,
    Assumptions,
    References,
    Tags,
    Variables,
    Related,
}

impl EditorField {
    pub const ALL: [EditorField; 8] = [
        EditorField::Name,
        EditorField::Description,
        EditorField::Latex,
        EditorField::Assumptions,
        EditorField::References,
        EditorField::Tags,
        EditorField::Variables,
        EditorField::Related,
    ];
    pub const COUNT: usize = Self::ALL.len();

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::COUNT]
    }

    pub fn prev(self) -> Self {
        Self::ALL[(self.index() + Self::COUNT - 1) % Self::COUNT]
    }

    pub fn is_list(self) -> bool {
        matches!(
            self,
            EditorField::References | EditorField::Variables | EditorField::Related
        )
    }

    pub fn is_multiline(self) -> bool {
        matches!(
            self,
            EditorField::Description | EditorField::Latex | EditorField::Assumptions
        )
    }
}

#[derive(Clone)]
pub struct EditorState {
    pub editing: Option<EquationId>,
    pub fields: [TextArea<'static>; EditorField::COUNT],
    pub focus: EditorField,
    pub related_cursor: usize,
    pub reference_cursor: usize,
    pub variable_cursor: usize,
    pub references: Vec<Reference>,
    pub variables: Vec<Variable>,
    pub reference_form: ReferenceForm,
    pub variable_form: VariableForm,
    pub dirty: bool,
    pub last_change: Instant,
    pub last_saved_signature: String,
    pub related_picker: RelatedPickerState,
    pub related: Vec<EquationId>,
}

impl EditorState {
    pub fn field(&self, field: EditorField) -> &TextArea<'static> {
        &self.fields[field.index()]
    }

    pub fn field_mut(&mut self, field: EditorField) -> &mut TextArea<'static> {
        &mut self.fields[field.index()]
    }

    pub fn field_text(&self, field: EditorField) -> String {
        textarea_text(self.field(field))
    }

    fn field_texts(&self) -> [String; EditorField::COUNT] {
        EditorField::ALL.map(|field| self.field_text(field))
    }

    fn set_field_text(&mut self, field: EditorField, text: String) {
        set_textarea_text(self.field_mut(field), text);
    }
}

pub const REFERENCE_FIELD_LABELS: [&str; 6] = ["Authors", "Year", "Title", "DOI", "URL", "Page(s)"];
pub const VARIABLE_FIELD_LABELS: [&str; 2] = ["Symbol", "Description"];
pub const QUANTITY_FIELD_LABELS: [&str; 4] = ["Symbol", "Name", "Description", "Unit(s)"];

#[derive(Clone)]
pub struct ReferenceForm {
    pub fields: [TextArea<'static>; 6],
    pub focus: usize,
    pub editing: Option<usize>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct VariableForm {
    pub fields: [TextArea<'static>; 2],
    pub focus: usize,
    pub editing: Option<usize>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct QuantityFormState {
    pub fields: [TextArea<'static>; 4],
    pub focus: usize,
    pub editing: Option<QuantityId>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct QuantityResolverState {
    pub queue: Vec<usize>,
    pub position: usize,
    pub query: String,
    pub query_cursor: usize,
    pub cursor: usize,
    pub linked: usize,
    pub created: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolverRow {
    CreateNew,
    Existing(QuantityId),
}

impl VariableForm {
    fn empty() -> Self {
        Self {
            fields: std::array::from_fn(|_| textarea_from_text("")),
            focus: 0,
            editing: None,
            error: None,
        }
    }

    fn from_variable(variable: &Variable, index: usize) -> Self {
        let values = [variable.symbol.clone(), variable.description.clone()];
        Self {
            fields: values.each_ref().map(|value| textarea_from_text(value)),
            focus: 0,
            editing: Some(index),
            error: None,
        }
    }

    fn text_at(&self, index: usize) -> String {
        textarea_text(&self.fields[index])
    }
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
            reference.pages.clone().unwrap_or_default(),
        ];
        Self {
            fields: values.each_ref().map(|value| textarea_from_text(value)),
            focus: 0,
            editing: Some(index),
            error: None,
        }
    }

    fn text_at(&self, index: usize) -> String {
        textarea_text(&self.fields[index])
    }
}

impl QuantityFormState {
    fn empty() -> Self {
        Self {
            fields: std::array::from_fn(|_| textarea_from_text("")),
            focus: 0,
            editing: None,
            error: None,
        }
    }

    fn from_quantity(quantity: &Quantity) -> Self {
        let values = [
            quantity.symbol.clone(),
            quantity.name.clone(),
            quantity.description.clone(),
            quantity.units.clone(),
        ];
        Self {
            fields: values.each_ref().map(|value| textarea_from_text(value)),
            focus: 0,
            editing: Some(quantity.id),
            error: None,
        }
    }

    fn text_at(&self, index: usize) -> String {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserFilter {
    None,
    Search(String),
    Tag(String),
    Untagged,
    Quantity { id: QuantityId, label: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserFilterFocus {
    Search,
    List,
}

#[derive(Clone)]
pub struct CmdlineState {
    pub input: String,
    pub cursor: usize,
    pub selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagPickerRow {
    Untagged { count: usize },
    Tag { name: String, count: usize },
}

pub struct AppState {
    pub store: Store,
    pub mode: Mode,
    pub all_items: Vec<EquationSummary>,
    pub items: Vec<EquationSummary>,
    pub tag_counts: Vec<(String, usize)>,
    pub quantities: Vec<(Quantity, usize)>,
    pub trash_items: Vec<TrashEntry>,
    pub trash_cursor: usize,
    pub tag_picker_cursor: usize,
    pub tag_picker_scroll_offset: usize,
    pub tag_picker_visible_height: u16,
    pub quantity_cursor: usize,
    pub quantity_scroll_offset: usize,
    pub quantity_visible_height: u16,
    pub untagged_count: usize,
    pub browser_filter: BrowserFilter,
    pub browser_filter_cursor: usize,
    pub browser_filter_focus: BrowserFilterFocus,
    pub vim_go_prefix: bool,
    pub cursor: usize,
    pub list_scroll_offset: usize,
    pub list_visible_height: u16,
    pub focus: Pane,
    pub layout: LayoutOrientation,
    pub should_quit: bool,
    pub help_open: bool,
    pub graphics_ok: bool,
    pub cell_size_px: TerminalCellSize,
    pub status: String,
    pub selected: Option<Equation>,
    pub editor: Option<EditorState>,
    pub quantity_form: Option<QuantityFormState>,
    pub quantity_resolver: Option<QuantityResolverState>,
    pub cmdline: Option<CmdlineState>,
    pub preview_protocol: Option<StatefulProtocol>,
    pub preview_error: Option<String>,
    pub preview_latex: String,
    pub preview_px: u32,
    pub preview_render_px: u32,
    pub preview_preserve_on_error: bool,
    pub notification: Option<Notification>,
    editor_history: Vec<EditorState>,
    render_queue: RenderQueue,
    protocol_warm_worker: ProtocolWarmWorker,
    pending_protocol_results: VecDeque<ProtocolWarmResult>,
    pending_queue_results: VecDeque<QueueResult>,
    generation: u64,
    dispatched_generation: u64,
    last_change: Instant,
    cache: HashMap<u64, RgbaImage>,
    cache_order: VecDeque<u64>,
    queued_keys: HashSet<u64>,
    render_failed: HashSet<u64>,
    needs_warm_submit: bool,
    protocol_warm_inflight: HashMap<u64, Size>,
    protocol_cache: HashMap<u64, StatefulProtocol>,
    protocol_cache_order: VecDeque<u64>,
    protocol_cache_epoch: u64,
    preview_cache_key: u64,
    preview_warm_size: Option<Size>,
    graphics: Graphics,
}

pub struct Notification {
    pub message: String,
    pub created_at: Instant,
    #[allow(dead_code)]
    pub is_error: bool,
    pub ttl: Duration,
}

impl Notification {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            created_at: Instant::now(),
            is_error: false,
            ttl: Duration::from_secs(3),
        }
    }

    #[allow(dead_code)]
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            created_at: Instant::now(),
            is_error: true,
            ttl: Duration::from_secs(6),
        }
    }

    pub fn expired(&self) -> bool {
        self.created_at.elapsed() >= self.ttl
    }
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
            tag_counts: Vec::new(),
            quantities: Vec::new(),
            trash_items: Vec::new(),
            trash_cursor: 0,
            tag_picker_cursor: 0,
            tag_picker_scroll_offset: 0,
            tag_picker_visible_height: 0,
            quantity_cursor: 0,
            quantity_scroll_offset: 0,
            quantity_visible_height: 0,
            untagged_count: 0,
            browser_filter: BrowserFilter::None,
            browser_filter_cursor: 0,
            browser_filter_focus: BrowserFilterFocus::Search,
            vim_go_prefix: false,
            cursor: 0,
            list_scroll_offset: 0,
            list_visible_height: 0,
            focus: Pane::List,
            layout: LayoutOrientation::Vertical,
            should_quit: false,
            help_open: false,
            graphics_ok,
            cell_size_px,
            status: "Ready".to_string(),
            selected: None,
            editor: None,
            quantity_form: None,
            quantity_resolver: None,
            cmdline: None,
            preview_protocol: None,
            preview_error: None,
            preview_latex: String::new(),
            preview_px: default_equation_px,
            preview_render_px: default_equation_px,
            preview_preserve_on_error: false,
            notification: None,
            editor_history: Vec::new(),
            render_queue: RenderQueue::spawn(),
            protocol_warm_worker: ProtocolWarmWorker::spawn(),
            pending_protocol_results: VecDeque::new(),
            pending_queue_results: VecDeque::new(),
            generation: 0,
            dispatched_generation: 0,
            last_change: Instant::now(),
            cache: HashMap::new(),
            cache_order: VecDeque::new(),
            queued_keys: HashSet::new(),
            render_failed: HashSet::new(),
            needs_warm_submit: false,
            protocol_warm_inflight: HashMap::new(),
            protocol_cache: HashMap::new(),
            protocol_cache_order: VecDeque::new(),
            protocol_cache_epoch: 0,
            preview_cache_key: 0,
            preview_warm_size: None,
            graphics,
        };
        app.reload()?;
        app.schedule_selected();
        Ok(app)
    }

    pub fn reload(&mut self) -> anyhow::Result<()> {
        let selected = self.selected_id();
        self.all_items = self.store.list()?;
        self.tag_counts = self.store.tag_counts()?;
        self.quantities = self.store.quantities()?;
        self.untagged_count = self.store.untagged_count()?;
        self.refresh_items()?;
        if let Some(id) = selected
            && let Some(index) = self.items.iter().position(|item| item.id == id)
        {
            self.cursor = index;
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

    pub fn selected_trash_id(&self) -> Option<EquationId> {
        self.trash_items.get(self.trash_cursor).map(|item| item.id)
    }

    pub fn browser_title(&self) -> String {
        match &self.browser_filter {
            BrowserFilter::None => "Equations".to_string(),
            BrowserFilter::Search(query) => format!("Search: {}", query),
            BrowserFilter::Tag(tag) => format!("Tag: {tag}"),
            BrowserFilter::Untagged => "Untagged".to_string(),
            BrowserFilter::Quantity { label, .. } => format!("Quantity: {label}"),
        }
    }

    pub fn tag_picker_rows(&self) -> Vec<TagPickerRow> {
        let mut rows = Vec::new();
        if self.untagged_count > 0 {
            rows.push(TagPickerRow::Untagged {
                count: self.untagged_count,
            });
        }
        let mut tags = self.tag_counts.clone();
        tags.sort_by_key(|(name, _)| name.to_lowercase());
        rows.extend(
            tags.into_iter()
                .map(|(name, count)| TagPickerRow::Tag { name, count }),
        );
        rows
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
        if self.preview_protocol.is_none()
            && let Some(display) = self.cache.get(&self.preview_cache_key).cloned()
        {
            self.queue_current_protocol_warm(self.preview_cache_key, display);
        }
        if matches!(self.mode, Mode::Browser | Mode::Search) {
            self.schedule_warm_neighbors();
        }
    }

    pub fn refresh_graphics_if_changed(&mut self) {
        let Some(graphics) = Graphics::probe(GRAPHICS_REFRESH_TIMEOUT) else {
            return;
        };
        if graphics.cell_size_px == self.cell_size_px && graphics.graphics_ok == self.graphics_ok {
            self.graphics = graphics;
            return;
        }

        self.graphics = graphics;
        self.graphics_ok = self.graphics.graphics_ok;
        self.cell_size_px = self.graphics.cell_size_px;
        self.invalidate_render_caches();
        self.status = format!(
            "Terminal cell size: {}x{} px",
            self.cell_size_px.width, self.cell_size_px.height
        );
        self.schedule_selected();
    }

    pub fn cache_status_for(&self, latex: &str, px: u32) -> CacheStatus {
        let key = render_cache::key(latex, self.effective_render_px(px));
        if self.queued_keys.contains(&key)
            || self.protocol_warm_inflight.contains_key(&key)
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
            BrowserFilter::Tag(tag) => self.store.by_tag(tag)?,
            BrowserFilter::Untagged => self.store.untagged()?,
            BrowserFilter::Quantity { id, .. } => self.store.by_quantity(*id)?,
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

    fn reload_trash(&mut self) -> anyhow::Result<()> {
        self.trash_items = self.store.list_trash()?;
        self.trash_cursor = self
            .trash_cursor
            .min(self.trash_items.len().saturating_sub(1));
        Ok(())
    }

    fn move_tag_picker_cursor_to(&mut self, cursor: usize) {
        let rows_len = self.tag_picker_rows().len();
        if rows_len == 0 {
            self.tag_picker_cursor = 0;
            self.tag_picker_scroll_offset = 0;
            return;
        }
        self.tag_picker_cursor = cursor.min(rows_len - 1);
        let visible = self.tag_picker_visible_height.max(1) as usize;
        if self.tag_picker_cursor < self.tag_picker_scroll_offset {
            self.tag_picker_scroll_offset = self.tag_picker_cursor;
        } else if self.tag_picker_cursor >= self.tag_picker_scroll_offset + visible {
            self.tag_picker_scroll_offset = self.tag_picker_cursor + 1 - visible;
        }
    }

    fn move_quantity_cursor_to(&mut self, cursor: usize) {
        if self.quantities.is_empty() {
            self.quantity_cursor = 0;
            self.quantity_scroll_offset = 0;
            return;
        }
        self.quantity_cursor = cursor.min(self.quantities.len() - 1);
        let visible = list_visible_item_count(self.quantity_visible_height).max(1);
        if self.quantity_cursor < self.quantity_scroll_offset {
            self.quantity_scroll_offset = self.quantity_cursor;
        } else if self.quantity_cursor >= self.quantity_scroll_offset + visible {
            self.quantity_scroll_offset = self.quantity_cursor + 1 - visible;
        }
    }

    fn move_browser_cursor_to(&mut self, cursor: usize) {
        if self.items.is_empty() {
            return;
        }
        let previous = self.cursor;
        self.cursor = cursor.min(self.items.len() - 1);
        let visible = list_visible_item_count(self.list_visible_height).max(1);
        if self.cursor < self.list_scroll_offset {
            self.list_scroll_offset = self.cursor;
        } else if self.cursor >= self.list_scroll_offset + visible {
            self.list_scroll_offset = self.cursor + 1 - visible;
        }
        if self.cursor != previous {
            self.schedule_selected_deferred();
        }
    }

    fn input_cmdline(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;

        let Some(cmdline) = &mut self.cmdline else {
            return;
        };
        match key.code {
            KeyCode::Char(ch) => {
                cmdline.input.insert(cmdline.cursor, ch);
                cmdline.cursor += ch.len_utf8();
                cmdline.selected = 0;
            }
            KeyCode::Backspace => {
                if cmdline.cursor > 0 {
                    let start = prev_boundary(&cmdline.input, cmdline.cursor);
                    cmdline.input.replace_range(start..cmdline.cursor, "");
                    cmdline.cursor = start;
                    cmdline.selected = 0;
                }
            }
            KeyCode::Delete => {
                if cmdline.cursor < cmdline.input.len() {
                    let end = next_boundary(&cmdline.input, cmdline.cursor);
                    cmdline.input.replace_range(cmdline.cursor..end, "");
                    cmdline.selected = 0;
                }
            }
            KeyCode::Up => cycle_cmdline_selection(cmdline, false),
            KeyCode::Down => cycle_cmdline_selection(cmdline, true),
            KeyCode::Left => {
                cmdline.cursor = prev_boundary(&cmdline.input, cmdline.cursor);
            }
            KeyCode::Right => {
                if cmdline.cursor == cmdline.input.len()
                    && selected_command(&cmdline.input, cmdline.selected).is_some()
                {
                    accept_cmdline_state(cmdline);
                } else {
                    cmdline.cursor = next_boundary(&cmdline.input, cmdline.cursor);
                }
            }
            KeyCode::Home => cmdline.cursor = 0,
            KeyCode::End => cmdline.cursor = cmdline.input.len(),
            _ => {}
        }
    }

    fn accept_cmdline(&mut self) {
        if let Some(cmdline) = &mut self.cmdline {
            accept_cmdline_state(cmdline);
        }
    }

    fn execute_cmdline(&mut self) {
        let input = self
            .cmdline
            .as_ref()
            .map(|cmdline| cmdline.input.clone())
            .unwrap_or_default();
        let selected = self
            .cmdline
            .as_ref()
            .map(|cmdline| cmdline.selected)
            .unwrap_or_default();
        let command = exact_command(&input).or_else(|| selected_command(&input, selected));

        self.cmdline = None;
        self.mode = Mode::Browser;
        self.force_preview_redraw();

        match command.and_then(command_action) {
            Some(action) => self.apply(action),
            None => {
                self.status = format!("Unknown command: {input}");
            }
        }
    }

    /// Force the live preview to re-encode and re-emit its graphics so they repaint
    /// over wherever a transient overlay (the cmdline) was drawn. `Clear` only wipes
    /// ratatui buffer cells — terminal-graphics images persist on screen until the
    /// protocol actually re-renders, so closing an overlay otherwise leaves artifacts.
    /// The decoded image stays in `self.cache`, so the re-encode is cheap.
    fn force_preview_redraw(&mut self) {
        self.preview_protocol = None;
        self.protocol_cache.remove(&self.preview_cache_key);
        self.dispatched_generation = self.generation.saturating_sub(1);
    }

    fn input_browser_filter(&mut self, key: crossterm::event::KeyEvent) -> anyhow::Result<()> {
        use crossterm::event::KeyCode;
        let query = match &mut self.browser_filter {
            BrowserFilter::Search(query) => query,
            BrowserFilter::None
            | BrowserFilter::Tag(_)
            | BrowserFilter::Untagged
            | BrowserFilter::Quantity { .. } => {
                return Ok(());
            }
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
                self.editor_history.clear();
                self.open_editor(None);
                Ok(())
            }
            Action::CopyCurrent => self.copy_current_equation(),
            Action::CopyLatexToClipboard => self.copy_selected_latex_to_clipboard(),
            Action::OpenReference => self.open_reference(),
            Action::OpenTags => {
                self.tag_picker_cursor = 0;
                self.tag_picker_scroll_offset = 0;
                self.mode = Mode::TagPicker;
                self.status = format!("Tags: {} tag(s)", self.tag_counts.len());
                Ok(())
            }
            Action::OpenQuantities => {
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
                if let Some((quantity, _)) = self.quantities.get(self.quantity_cursor) {
                    self.browser_filter = BrowserFilter::Quantity {
                        id: quantity.id,
                        label: quantity_label(quantity),
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
            Action::QuantityPickerCancel => {
                self.mode = Mode::Browser;
                self.schedule_selected();
                Ok(())
            }
            Action::DeleteRequest => {
                if let Some(id) = self.selected_id() {
                    self.mode = Mode::ConfirmDelete(id);
                }
                Ok(())
            }
            Action::OpenCmdline => {
                self.cmdline = Some(CmdlineState {
                    input: String::new(),
                    cursor: 0,
                    selected: 0,
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
                self.cmdline = None;
                self.mode = Mode::Browser;
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
                    Mode::ConfirmPurge(_) => Mode::Trash,
                    Mode::ConfirmRemoveQuantity(_) => Mode::QuantityPicker,
                    Mode::ConfirmRemoveRelated(_) => Mode::Editor,
                    Mode::ConfirmRemoveReference(_) => Mode::Editor,
                    Mode::ConfirmRemoveVariable(_) => Mode::Editor,
                    Mode::ReferenceEditor => Mode::Editor,
                    Mode::VariableEditor => Mode::Editor,
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
                    Mode::Search | Mode::Cmdline | Mode::Browser => Mode::Browser,
                    Mode::Trash => Mode::Browser,
                    Mode::TagPicker => Mode::Browser,
                    Mode::QuantityPicker => Mode::Browser,
                    Mode::QuantityForm => Mode::QuantityPicker,
                    Mode::QuantityResolver => Mode::Editor,
                };
                self.schedule_selected();
                Ok(())
            }
            Action::EditorNextField => {
                if let Some(editor) = &mut self.editor {
                    editor.focus = editor.focus.next();
                }
                Ok(())
            }
            Action::EditorPrevField => {
                if let Some(editor) = &mut self.editor {
                    editor.focus = editor.focus.prev();
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

    fn report_error(&mut self, err: impl std::fmt::Display) {
        self.status = err.to_string();
        self.notification = Some(Notification::error(self.status.clone()));
    }

    pub fn tick_render(&mut self) {
        let started = Instant::now();
        if self
            .notification
            .as_ref()
            .is_some_and(Notification::expired)
        {
            self.notification = None;
        }

        self.collect_worker_results();
        self.process_current_preview_results();
        self.process_protocol_results(started);
        self.process_queue_results(started);

        if self.generation != self.dispatched_generation
            && self.last_change.elapsed() >= Duration::from_millis(150)
        {
            let new_key = render_cache::key(&self.preview_latex, self.preview_render_px);
            if let Some(display) = self.cache.get(&new_key).cloned() {
                if self.queue_current_protocol_warm(new_key, display) {
                    self.preview_error = None;
                    self.dispatched_generation = self.generation;
                }
            } else {
                self.submit_to_queue(
                    new_key,
                    self.preview_latex.clone(),
                    self.preview_render_px,
                    0,
                );
                self.dispatched_generation = self.generation;
            }
        }

        if self.needs_warm_submit
            && matches!(self.mode, Mode::Browser | Mode::Cmdline | Mode::Search)
            && self.last_change.elapsed() >= Duration::from_millis(100)
        {
            self.schedule_warm_neighbors();
            self.needs_warm_submit = false;
        }

        if matches!(self.mode, Mode::Editor)
            && self.editor.as_ref().is_some_and(|editor| {
                editor.dirty && editor.last_change.elapsed() >= Duration::from_millis(300)
            })
            && let Err(err) = self.persist_editor(false)
        {
            self.status = err.to_string();
            if let Some(editor) = &mut self.editor {
                editor.last_change = Instant::now();
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
            let Some(result) = self.render_queue.try_recv() else {
                break;
            };
            self.pending_queue_results.push_back(result);
        }
    }

    fn process_current_preview_results(&mut self) {
        let preview_key = self.preview_cache_key;
        if let Some(index) = self
            .pending_protocol_results
            .iter()
            .position(|result| protocol_result_key(result) == Some(preview_key))
            && let Some(result) = self.pending_protocol_results.remove(index)
        {
            self.handle_protocol_result(result);
        }
        if let Some(index) = self
            .pending_queue_results
            .iter()
            .position(|result| result.key == preview_key)
            && let Some(result) = self.pending_queue_results.remove(index)
        {
            self.handle_queue_result(result);
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

    fn process_queue_results(&mut self, started: Instant) {
        for _ in 0..QUEUE_RESULTS_PER_TICK {
            if result_budget_spent(started) {
                break;
            }
            let Some(result) = self.pending_queue_results.pop_front() else {
                break;
            };
            self.handle_queue_result(result);
        }
    }

    fn handle_protocol_result(&mut self, result: ProtocolWarmResult) {
        match result.outcome {
            ProtocolWarmOutcome::Ready {
                epoch,
                key,
                size,
                protocol,
            } => {
                if epoch != self.protocol_cache_epoch {
                    return;
                }
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
            ProtocolWarmOutcome::Failed { epoch, key, size } => {
                if epoch != self.protocol_cache_epoch {
                    return;
                }
                self.remove_protocol_inflight(key, size);
                if key == self.preview_cache_key
                    && self.preview_warm_size == Some(size)
                    && self.preview_protocol.is_none()
                {
                    self.dispatched_generation = self.generation.saturating_sub(1);
                }
            }
            ProtocolWarmOutcome::Skipped(jobs) => {
                for (epoch, key, size) in jobs {
                    if epoch != self.protocol_cache_epoch {
                        continue;
                    }
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

    fn submit_to_queue(&mut self, key: u64, latex: String, px: u32, priority: u8) {
        if self.queued_keys.contains(&key) || self.render_failed.contains(&key) {
            return;
        }
        self.render_queue.submit(QueueJob {
            key,
            latex,
            px,
            priority,
        });
        self.queued_keys.insert(key);
    }

    fn handle_queue_result(&mut self, result: QueueResult) {
        self.queued_keys.remove(&result.key);
        let is_current = result.key == self.preview_cache_key;
        match result.image {
            Err(err) => {
                self.render_failed.insert(result.key);
                if is_current {
                    self.preview_error = Some(err);
                    if !self.preview_preserve_on_error {
                        self.preview_protocol = None;
                    }
                    self.dispatched_generation = self.generation;
                }
            }
            Ok(raw) => {
                let display = self.graphics.recolor(raw);
                if is_current {
                    self.preview_error = None;
                    if self.preview_protocol.is_none() {
                        if let Some(protocol) = self.take_protocol(result.key) {
                            self.preview_protocol = Some(protocol);
                        } else {
                            self.queue_current_protocol_warm(result.key, display.clone());
                        }
                    }
                    self.dispatched_generation = self.generation;
                } else {
                    self.queue_protocol_warm_inner(result.key, display.clone(), false);
                }
                self.remember_cache(result.key, display);
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
                | Mode::VariableEditor
                | Mode::ConfirmRemoveVariable(_)
                | Mode::QuantityResolver
        );
        let latex = if in_editor {
            self.editor
                .as_ref()
                .map(|editor| editor.field_text(EditorField::Latex))
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
            if immediate {
                self.schedule_warm_neighbors();
            } else {
                self.needs_warm_submit = true;
            }
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
                | Mode::VariableEditor
                | Mode::ConfirmRemoveVariable(_)
                | Mode::QuantityResolver
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
            return;
        }

        // An already-encoded protocol can be shown without any work on the UI thread.
        if let Some(protocol) = self.take_protocol(new_key) {
            self.preview_protocol = Some(protocol);
            self.preview_error = None;
            self.dispatched_generation = self.generation;
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
            }
            // If no preview size is known yet, set_preview_warm_size kicks the encode once
            // the pane is first drawn; until then the debounced full render is the fallback.
            return;
        }

        // Image not in any cache — submit to the render queue.
        self.submit_to_queue(
            new_key,
            self.preview_latex.clone(),
            self.preview_render_px,
            0,
        );
        self.dispatched_generation = self.generation;
    }

    fn schedule_warm_neighbors(&mut self) {
        if self.items.is_empty() {
            return;
        }

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
                if !seen.insert(key) {
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

                // Not rendered yet — submit to the render queue.
                self.submit_to_queue(key, item.latex.clone(), item_px, distance as u8);
            }
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
            epoch: self.protocol_cache_epoch,
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
            epoch: self.protocol_cache_epoch,
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
        let initial_variables = equation
            .as_ref()
            .map(|eq| eq.variables.clone())
            .unwrap_or_default();
        let field_values = EditorField::ALL.map(|field| match (&equation, field) {
            (Some(eq), EditorField::Name) => eq.name.clone(),
            (Some(eq), EditorField::Description) => eq.description.clone(),
            (Some(eq), EditorField::Latex) => eq.latex.clone(),
            (Some(eq), EditorField::Assumptions) => eq.assumptions.clone(),
            (Some(_), EditorField::References) => String::new(),
            (Some(eq), EditorField::Tags) => eq.tags.join(", "),
            (Some(_), EditorField::Variables) => format_variables(&initial_variables),
            (Some(_), EditorField::Related) => format_related(&initial_related, &self.all_items),
            (None, _) => String::new(),
        });
        let fields = field_values
            .each_ref()
            .map(|value| textarea_from_text(value));
        self.editor = Some(EditorState {
            editing: id,
            last_saved_signature: fields_signature(
                &field_values,
                &initial_related,
                &initial_references,
                &initial_variables,
            ),
            fields,
            focus: EditorField::Name,
            related_cursor: 0,
            reference_cursor: 0,
            variable_cursor: 0,
            references: initial_references,
            variables: initial_variables,
            reference_form: ReferenceForm::empty(),
            variable_form: VariableForm::empty(),
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
            KeyCode::Char('r') if focused == EditorField::Related => {
                self.open_related_picker();
                return;
            }
            KeyCode::Char('a') if focused == EditorField::References => {
                self.open_reference_form(None);
                return;
            }
            KeyCode::Char('a') if focused == EditorField::Variables => {
                self.open_variable_form(None);
                return;
            }
            KeyCode::Char('k') if focused == EditorField::References => {
                editor.reference_cursor = editor.reference_cursor.saturating_sub(1);
                return;
            }
            KeyCode::Char('j') if focused == EditorField::References => {
                let max = editor.references.len().saturating_sub(1);
                editor.reference_cursor = (editor.reference_cursor + 1).min(max);
                return;
            }
            KeyCode::Char('d') if focused == EditorField::References => {
                if let Some(idx) = self.current_reference_index() {
                    self.mode = Mode::ConfirmRemoveReference(idx);
                }
                return;
            }
            KeyCode::Char('k') if focused == EditorField::Variables => {
                editor.variable_cursor = editor.variable_cursor.saturating_sub(1);
                return;
            }
            KeyCode::Char('j') if focused == EditorField::Variables => {
                let max = editor.variables.len().saturating_sub(1);
                editor.variable_cursor = (editor.variable_cursor + 1).min(max);
                return;
            }
            KeyCode::Char('d') if focused == EditorField::Variables => {
                if let Some(idx) = self.current_variable_index() {
                    self.mode = Mode::ConfirmRemoveVariable(idx);
                }
                return;
            }
            KeyCode::Char('c') if focused == EditorField::Variables => {
                if let Err(err) = self.link_variables_to_quantities() {
                    self.report_error(err);
                }
                return;
            }
            KeyCode::Char('u') if focused == EditorField::Variables => {
                if let Some(index) = self.current_variable_index()
                    && let Some(editor) = &mut self.editor
                {
                    editor.variables[index].quantity_id = None;
                    mark_editor_dirty(editor);
                }
                return;
            }
            KeyCode::Char('k') if focused == EditorField::Related => {
                editor.related_cursor = editor.related_cursor.saturating_sub(1);
                return;
            }
            KeyCode::Char('j') if focused == EditorField::Related => {
                let max = editor.related.len().saturating_sub(1);
                editor.related_cursor = (editor.related_cursor + 1).min(max);
                return;
            }
            KeyCode::Char('d') if focused == EditorField::Related => {
                if let Some(id) = self.current_related_id() {
                    self.mode = Mode::ConfirmRemoveRelated(id);
                }
                return;
            }
            KeyCode::Enter if matches!(focused, EditorField::Name | EditorField::Tags) => return,
            KeyCode::Enter if focused == EditorField::References => {
                if let Some(idx) = self.current_reference_index() {
                    self.open_reference_form(Some(idx));
                }
                return;
            }
            KeyCode::Enter if focused == EditorField::Variables => {
                if let Some(idx) = self.current_variable_index() {
                    self.open_variable_form(Some(idx));
                }
                return;
            }
            KeyCode::Enter if focused == EditorField::Related => {
                self.open_selected_related_detail();
                return;
            }
            _ if !focused.is_list() && editor.field_mut(focused).input(key) => {}
            _ => return,
        }
        mark_editor_dirty(editor);
        if editor.focus == EditorField::Latex {
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
        if editor.focus != EditorField::Related {
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
        if editor.focus != EditorField::References || editor.references.is_empty() {
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
        let authors = editor.reference_form.text_at(0).trim().to_string();
        let year_raw = editor.reference_form.text_at(1).trim().to_string();
        let title = editor.reference_form.text_at(2).trim().to_string();
        let doi_raw = editor.reference_form.text_at(3).trim().to_string();
        let url_raw = editor.reference_form.text_at(4).trim().to_string();
        let pages_raw = editor.reference_form.text_at(5).trim().to_string();

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
        let pages = if pages_raw.is_empty() {
            None
        } else {
            match normalize_pages(&pages_raw) {
                Some(pages) => Some(pages),
                None => {
                    editor.reference_form.error =
                        Some("Pages must look like 1, 5-7, or 2, 5-7".to_string());
                    return;
                }
            }
        };

        let reference = Reference {
            authors,
            year,
            title,
            doi,
            url,
            pages,
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
        if let Some(editor) = &mut self.editor
            && index < editor.references.len()
        {
            editor.references.remove(index);
            editor.reference_cursor = editor
                .reference_cursor
                .min(editor.references.len().saturating_sub(1));
            mark_editor_dirty(editor);
        }
    }

    fn current_variable_index(&self) -> Option<usize> {
        let editor = self.editor.as_ref()?;
        if editor.focus != EditorField::Variables || editor.variables.is_empty() {
            return None;
        }
        Some(editor.variable_cursor.min(editor.variables.len() - 1))
    }

    fn open_variable_form(&mut self, index: Option<usize>) {
        let form = {
            let Some(editor) = self.editor.as_ref() else {
                return;
            };
            match index {
                Some(i) if i < editor.variables.len() => {
                    VariableForm::from_variable(&editor.variables[i], i)
                }
                _ => VariableForm::empty(),
            }
        };
        if let Some(editor) = self.editor.as_mut() {
            editor.variable_form = form;
        }
        self.mode = Mode::VariableEditor;
    }

    fn input_variable_form(&mut self, key: crossterm::event::KeyEvent) {
        if let Some(editor) = &mut self.editor {
            let focus = editor.variable_form.focus;
            editor.variable_form.fields[focus].input(key);
            editor.variable_form.error = None;
        }
    }

    fn variable_form_next_field(&mut self) {
        if let Some(editor) = &mut self.editor {
            let n = editor.variable_form.fields.len();
            editor.variable_form.focus = (editor.variable_form.focus + 1) % n;
        }
    }

    fn variable_form_prev_field(&mut self) {
        if let Some(editor) = &mut self.editor {
            let n = editor.variable_form.fields.len();
            editor.variable_form.focus = (editor.variable_form.focus + n - 1) % n;
        }
    }

    fn input_quantity_form(&mut self, key: crossterm::event::KeyEvent) {
        if let Some(form) = &mut self.quantity_form {
            let focus = form.focus;
            form.fields[focus].input(key);
            form.error = None;
        }
    }

    fn quantity_form_next_field(&mut self) {
        if let Some(form) = &mut self.quantity_form {
            form.focus = (form.focus + 1) % form.fields.len();
        }
    }

    fn quantity_form_prev_field(&mut self) {
        if let Some(form) = &mut self.quantity_form {
            form.focus = form.focus.checked_sub(1).unwrap_or(form.fields.len() - 1);
        }
    }

    fn save_quantity_form(&mut self) -> anyhow::Result<()> {
        let Some(form) = &mut self.quantity_form else {
            return Ok(());
        };
        let symbol = form.text_at(0).trim().to_string();
        if symbol.is_empty() {
            form.error = Some("Enter a symbol".to_string());
            return Ok(());
        }
        let mut quantity = form
            .editing
            .and_then(|id| {
                self.quantities
                    .iter()
                    .find(|(quantity, _)| quantity.id == id)
                    .map(|(quantity, _)| quantity.clone())
            })
            .unwrap_or_else(|| Quantity::new(symbol.clone()));
        quantity.symbol = symbol;
        quantity.name = form.text_at(1).trim().to_string();
        quantity.description = form.text_at(2).trim().to_string();
        quantity.units = form.text_at(3).trim().to_string();

        if form.editing.is_some() {
            self.store.update_quantity(&quantity)?;
        } else {
            self.store.insert_quantity(&quantity)?;
        }
        self.quantities = self.store.quantities()?;
        if let Some(index) = self
            .quantities
            .iter()
            .position(|(candidate, _)| candidate.id == quantity.id)
        {
            self.move_quantity_cursor_to(index);
        }
        self.quantity_form = None;
        self.mode = Mode::QuantityPicker;
        self.status = "Quantity saved".to_string();
        Ok(())
    }

    fn save_variable_form(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let symbol = editor.variable_form.text_at(0).trim().to_string();
        let description = editor.variable_form.text_at(1).trim().to_string();
        if symbol.is_empty() {
            editor.variable_form.error = Some("Enter a symbol".to_string());
            return;
        }
        if editor
            .variables
            .iter()
            .enumerate()
            .any(|(index, variable)| {
                Some(index) != editor.variable_form.editing && variable.symbol == symbol
            })
        {
            editor.variable_form.error = Some("Symbol already exists".to_string());
            return;
        }

        let variable = Variable {
            symbol,
            description,
            quantity_id: editor
                .variable_form
                .editing
                .and_then(|i| editor.variables.get(i))
                .and_then(|variable| variable.quantity_id),
        };
        let editing = editor.variable_form.editing;
        match editing {
            Some(i) if i < editor.variables.len() => editor.variables[i] = variable,
            _ => editor.variables.push(variable),
        }
        editor.variable_cursor = match editing {
            Some(i) => i.min(editor.variables.len().saturating_sub(1)),
            None => editor.variables.len().saturating_sub(1),
        };
        let display = format_variables(&editor.variables);
        editor.set_field_text(EditorField::Variables, display);
        mark_editor_dirty(editor);
        self.mode = Mode::Editor;
    }

    pub fn link_variables_to_quantities(&mut self) -> anyhow::Result<()> {
        let Some(editor) = self.editor.as_ref() else {
            return Ok(());
        };
        if editor.focus != EditorField::Variables {
            return Ok(());
        }
        let known: HashSet<_> = self
            .quantities
            .iter()
            .map(|(quantity, _)| quantity.id)
            .collect();
        let variables = editor
            .variables
            .iter()
            .enumerate()
            .filter(|(_, variable)| !variable.quantity_id.is_some_and(|id| known.contains(&id)))
            .map(|(index, variable)| {
                (
                    index,
                    variable.symbol.trim().to_string(),
                    variable.description.clone(),
                )
            })
            .filter(|(_, symbol, _)| !symbol.is_empty())
            .collect::<Vec<_>>();

        let mut links = Vec::new();
        let mut queue = Vec::new();
        let mut linked = 0;
        let mut created = 0;
        for (index, symbol, description) in variables {
            let matches = self.store.quantities_by_symbol(&symbol)?;
            match matches.as_slice() {
                [] => {
                    let mut quantity = Quantity::new(symbol);
                    quantity.description = description;
                    self.store.insert_quantity(&quantity)?;
                    links.push((index, quantity.id));
                    created += 1;
                }
                [quantity] => {
                    links.push((index, quantity.id));
                    linked += 1;
                }
                _ => queue.push(index),
            }
        }
        if created > 0 {
            self.quantities = self.store.quantities()?;
        }
        if let Some(editor) = &mut self.editor {
            for (index, id) in links {
                if let Some(variable) = editor.variables.get_mut(index) {
                    variable.quantity_id = Some(id);
                }
            }
            if linked > 0 || created > 0 {
                mark_editor_dirty(editor);
            }
        }
        if queue.is_empty() {
            self.status = format!("Variables linked: {linked} linked, {created} created");
            return Ok(());
        }
        let cursor = self.initial_resolver_cursor(queue[0]);
        self.quantity_resolver = Some(QuantityResolverState {
            queue,
            position: 0,
            query: String::new(),
            query_cursor: 0,
            cursor,
            linked,
            created,
            skipped: 0,
        });
        self.mode = Mode::QuantityResolver;
        Ok(())
    }

    pub fn resolver_rows(&self) -> Vec<ResolverRow> {
        let Some(resolver) = &self.quantity_resolver else {
            return Vec::new();
        };
        let Some(editor) = &self.editor else {
            return Vec::new();
        };
        let Some(index) = resolver.queue.get(resolver.position).copied() else {
            return Vec::new();
        };
        let Some(variable) = editor.variables.get(index) else {
            return Vec::new();
        };
        let symbol = variable.symbol.trim();
        let mut rows = vec![ResolverRow::CreateNew];
        let exact = self
            .quantities
            .iter()
            .filter(|(quantity, _)| quantity.symbol == symbol)
            .map(|(quantity, _)| quantity.id)
            .collect::<Vec<_>>();
        rows.extend(exact.iter().copied().map(ResolverRow::Existing));
        let exact: HashSet<_> = exact.into_iter().collect();
        let query = resolver.query.trim().to_lowercase();
        rows.extend(
            self.quantities
                .iter()
                .filter(|(quantity, _)| !exact.contains(&quantity.id))
                .filter(|(quantity, _)| {
                    query.is_empty()
                        || format!(
                            "{} {} {}",
                            quantity.symbol, quantity.name, quantity.description
                        )
                        .to_lowercase()
                        .contains(&query)
                })
                .map(|(quantity, _)| ResolverRow::Existing(quantity.id)),
        );
        rows
    }

    fn initial_resolver_cursor(&self, variable_index: usize) -> usize {
        let Some(editor) = &self.editor else {
            return 0;
        };
        let Some(variable) = editor.variables.get(variable_index) else {
            return 0;
        };
        if self
            .quantities
            .iter()
            .any(|(quantity, _)| quantity.symbol == variable.symbol.trim())
        {
            1
        } else {
            0
        }
    }

    fn move_resolver_cursor(&mut self, down: bool) {
        let max = self.resolver_rows().len().saturating_sub(1);
        if let Some(resolver) = &mut self.quantity_resolver {
            resolver.cursor = if down {
                (resolver.cursor + 1).min(max)
            } else {
                resolver.cursor.saturating_sub(1)
            };
        }
    }

    fn input_resolver(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        let Some(resolver) = &mut self.quantity_resolver else {
            return;
        };
        let mut changed = false;
        match key.code {
            KeyCode::Char(ch) => {
                resolver.query.insert(resolver.query_cursor, ch);
                resolver.query_cursor += ch.len_utf8();
                changed = true;
            }
            KeyCode::Backspace if resolver.query_cursor > 0 => {
                let previous = prev_boundary(&resolver.query, resolver.query_cursor);
                resolver.query.drain(previous..resolver.query_cursor);
                resolver.query_cursor = previous;
                changed = true;
            }
            KeyCode::Delete if resolver.query_cursor < resolver.query.len() => {
                let next = next_boundary(&resolver.query, resolver.query_cursor);
                resolver.query.drain(resolver.query_cursor..next);
                changed = true;
            }
            KeyCode::Left => {
                resolver.query_cursor = prev_boundary(&resolver.query, resolver.query_cursor)
            }
            KeyCode::Right => {
                resolver.query_cursor = next_boundary(&resolver.query, resolver.query_cursor)
            }
            KeyCode::Home => resolver.query_cursor = 0,
            KeyCode::End => resolver.query_cursor = resolver.query.len(),
            _ => {}
        }
        if changed {
            let index = self
                .quantity_resolver
                .as_ref()
                .and_then(|resolver| resolver.queue.get(resolver.position).copied());
            let cursor = index
                .map(|index| self.initial_resolver_cursor(index))
                .unwrap_or(0);
            let max = self.resolver_rows().len().saturating_sub(1);
            if let Some(resolver) = &mut self.quantity_resolver {
                resolver.cursor = cursor.min(max);
            }
        }
    }

    fn accept_resolver(&mut self) -> anyhow::Result<()> {
        let row = self
            .resolver_rows()
            .get(
                self.quantity_resolver
                    .as_ref()
                    .map(|resolver| resolver.cursor)
                    .unwrap_or(0),
            )
            .cloned()
            .unwrap_or(ResolverRow::CreateNew);
        let Some((variable_index, symbol, description)) = self.current_resolver_variable() else {
            return Ok(());
        };
        let quantity_id = match row {
            ResolverRow::CreateNew => {
                let mut quantity = Quantity::new(symbol);
                quantity.description = description;
                let id = quantity.id;
                self.store.insert_quantity(&quantity)?;
                self.quantities = self.store.quantities()?;
                if let Some(resolver) = &mut self.quantity_resolver {
                    resolver.created += 1;
                }
                id
            }
            ResolverRow::Existing(id) => {
                if let Some(resolver) = &mut self.quantity_resolver {
                    resolver.linked += 1;
                }
                id
            }
        };
        if let Some(editor) = &mut self.editor
            && let Some(variable) = editor.variables.get_mut(variable_index)
        {
            variable.quantity_id = Some(quantity_id);
            mark_editor_dirty(editor);
        }
        self.advance_resolver();
        Ok(())
    }

    fn current_resolver_variable(&self) -> Option<(usize, String, String)> {
        let resolver = self.quantity_resolver.as_ref()?;
        let editor = self.editor.as_ref()?;
        let index = resolver.queue.get(resolver.position).copied()?;
        let variable = editor.variables.get(index)?;
        Some((
            index,
            variable.symbol.trim().to_string(),
            variable.description.clone(),
        ))
    }

    fn advance_resolver(&mut self) {
        let Some(mut resolver) = self.quantity_resolver.take() else {
            return;
        };
        resolver.position += 1;
        if resolver.position >= resolver.queue.len() {
            self.mode = Mode::Editor;
            self.status = format!(
                "Variables linked: {} linked, {} created, {} skipped",
                resolver.linked, resolver.created, resolver.skipped
            );
            return;
        }
        resolver.query.clear();
        resolver.query_cursor = 0;
        resolver.cursor = self.initial_resolver_cursor(resolver.queue[resolver.position]);
        self.quantity_resolver = Some(resolver);
    }

    fn remove_variable(&mut self, index: usize) {
        if let Some(editor) = &mut self.editor
            && index < editor.variables.len()
        {
            editor.variables.remove(index);
            editor.variable_cursor = editor
                .variable_cursor
                .min(editor.variables.len().saturating_sub(1));
            let display = format_variables(&editor.variables);
            editor.set_field_text(EditorField::Variables, display);
            mark_editor_dirty(editor);
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
        editor.set_field_text(EditorField::Related, display);
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
        if focus != EditorField::Related {
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
        if editor.focus != EditorField::Related {
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
        editor.set_field_text(EditorField::Related, display);
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
        clone.assumptions = source.assumptions;
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
        self.notification = Some(Notification::info("equation copied"));
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
        self.notification = Some(Notification::info("latex copied"));
        Ok(())
    }

    fn open_reference(&mut self) -> anyhow::Result<()> {
        let Some(target) = self.current_reference_target()? else {
            self.status = "No reference URL for selection".to_string();
            return Ok(());
        };
        open_reference_target(&target)?;
        self.status = format!("Opened reference: {target}");
        self.notification = Some(Notification::info("reference opened"));
        Ok(())
    }

    fn current_reference_target(&mut self) -> anyhow::Result<Option<String>> {
        if let Some(editor) = self.editor.as_ref()
            && editor.focus == EditorField::References
        {
            let target = self
                .current_reference_index()
                .and_then(|index| editor.references.get(index))
                .and_then(reference_link);
            return Ok(target);
        }

        if let Some(target) = self
            .selected
            .as_ref()
            .and_then(|equation| equation.references.first())
            .and_then(reference_link)
        {
            return Ok(Some(target));
        }

        let Some(id) = self.selected_id() else {
            return Ok(None);
        };
        Ok(self
            .store
            .get(id)?
            .references
            .first()
            .and_then(reference_link))
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
        let variables = editor.variables.clone();
        if fields[EditorField::Name.index()].trim().is_empty() {
            return Ok(());
        }
        if fields[EditorField::Latex.index()].trim().is_empty() {
            return Ok(());
        }
        let signature = fields_signature(&fields, &related_ids, &references, &variables);
        if signature == last_saved_signature {
            if let Some(editor) = &mut self.editor {
                editor.dirty = false;
            }
            return Ok(());
        }
        validate_latex(&fields[EditorField::Latex.index()]).map_err(anyhow::Error::msg)?;
        let mut equation = if let Some(id) = editing {
            self.store.get(id)?
        } else {
            let mut equation = Equation::new(
                fields[EditorField::Name.index()].trim().to_string(),
                fields[EditorField::Latex.index()].trim().to_string(),
            );
            equation.px_height = self.default_equation_px();
            equation
        };
        equation.name = fields[EditorField::Name.index()].trim().to_string();
        equation.description = fields[EditorField::Description.index()].trim().to_string();
        equation.latex = fields[EditorField::Latex.index()].trim().to_string();
        equation.assumptions = fields[EditorField::Assumptions.index()].trim().to_string();
        equation.references = references.clone();
        equation.tags = {
            let mut seen = std::collections::HashSet::new();
            fields[EditorField::Tags.index()]
                .split(',')
                .map(str::trim)
                .filter(|tag| !tag.is_empty())
                .filter(|tag| seen.insert(*tag))
                .map(ToOwned::to_owned)
                .collect()
        };
        equation.variables = variables.clone();
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
        self.notification = Some(Notification::info("equation saved"));
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

    fn invalidate_render_caches(&mut self) {
        self.protocol_cache_epoch = self.protocol_cache_epoch.saturating_add(1);
        self.preview_protocol = None;
        self.cache.clear();
        self.cache_order.clear();
        self.protocol_cache.clear();
        self.protocol_cache_order.clear();
        self.protocol_warm_inflight.clear();
        self.pending_protocol_results.clear();
        self.pending_queue_results.clear();
        self.needs_warm_submit = true;
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
        if let Some(selected) = &mut self.selected
            && selected.id == id
        {
            selected.px_height = new_px;
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

pub fn quantity_label(quantity: &Quantity) -> String {
    if quantity.name.trim().is_empty() {
        quantity.symbol.clone()
    } else {
        format!("{} - {}", quantity.symbol, quantity.name)
    }
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
    fields: &[String; EditorField::COUNT],
    related: &[EquationId],
    references: &[Reference],
    variables: &[Variable],
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
                "{}|{}|{}|{}|{}|{}",
                r.authors,
                r.year.map(|y| y.to_string()).unwrap_or_default(),
                r.title,
                r.doi.clone().unwrap_or_default(),
                r.url.clone().unwrap_or_default(),
                r.pages.clone().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    let variables_part = variables
        .iter()
        .map(|v| {
            format!(
                "{}|{}|{}",
                v.symbol,
                v.description,
                v.quantity_id.map(|id| id.to_string()).unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        fields.join("\u{1f}"),
        related_part,
        references_part,
        variables_part
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

fn open_reference_target(target: &str) -> anyhow::Result<()> {
    let target = target.trim();
    if target.is_empty() {
        anyhow::bail!("Reference URL is empty");
    }
    if !is_supported_reference_target(target) {
        anyhow::bail!("Reference URL must be https or a local file");
    }
    let target = expand_home_path(target);
    platform_open(&target)?;
    Ok(())
}

fn is_supported_reference_target(target: &str) -> bool {
    target.starts_with("https://") || target.starts_with("file://") || !target.contains("://")
}

fn expand_home_path(target: &str) -> String {
    if let Some(rest) = target.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        let mut path = std::path::PathBuf::from(home);
        path.push(rest);
        return path.to_string_lossy().into_owned();
    }
    target.to_string()
}

#[cfg(target_os = "macos")]
fn platform_open(target: &str) -> anyhow::Result<()> {
    spawn_discarding_output(Command::new("open").arg(target))?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn platform_open(target: &str) -> anyhow::Result<()> {
    spawn_discarding_output(Command::new("cmd").args(["/C", "start", "", target]))?;
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_open(target: &str) -> anyhow::Result<()> {
    spawn_discarding_output(Command::new("xdg-open").arg(target))?;
    Ok(())
}

fn spawn_discarding_output(command: &mut Command) -> anyhow::Result<()> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
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

const COMMANDS: [&str; 8] = [
    "delete",
    "exit",
    "new",
    "openReference",
    "quantities",
    "search",
    "tags",
    "trash",
];

pub fn command_matches(prefix: &str) -> Vec<&'static str> {
    COMMANDS
        .iter()
        .copied()
        .filter(|command| {
            command.starts_with(prefix) || command_matches_ignore_case(command, prefix)
        })
        .collect()
}

fn selected_command(prefix: &str, selected: usize) -> Option<&'static str> {
    let matches = command_matches(prefix);
    matches
        .get(selected.min(matches.len().saturating_sub(1)))
        .copied()
}

fn exact_command(input: &str) -> Option<&'static str> {
    COMMANDS
        .iter()
        .copied()
        .find(|command| command.eq_ignore_ascii_case(input))
}

fn command_action(command: &str) -> Option<Action> {
    match command {
        "delete" => Some(Action::DeleteRequest),
        "exit" => Some(Action::Quit),
        "new" => Some(Action::NewEquation),
        "openReference" => Some(Action::OpenReference),
        "quantities" => Some(Action::OpenQuantities),
        "search" => Some(Action::StartSearch),
        "tags" => Some(Action::OpenTags),
        "trash" => Some(Action::OpenTrash),
        _ => None,
    }
}

fn accept_cmdline_state(cmdline: &mut CmdlineState) {
    if let Some(command) = selected_command(&cmdline.input, cmdline.selected) {
        cmdline.input = command.to_string();
        cmdline.cursor = cmdline.input.len();
        cmdline.selected = 0;
    }
}

fn cycle_cmdline_selection(cmdline: &mut CmdlineState, forward: bool) {
    let count = command_matches(&cmdline.input).len();
    if count == 0 {
        cmdline.selected = 0;
    } else if forward {
        cmdline.selected = (cmdline.selected + 1) % count;
    } else {
        cmdline.selected = cmdline.selected.checked_sub(1).unwrap_or(count - 1);
    }
}

fn command_matches_ignore_case(command: &str, prefix: &str) -> bool {
    prefix.len() <= command.len() && command[..prefix.len()].eq_ignore_ascii_case(prefix)
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
        CmdlineState, EditorField, command_matches, default_equation_px, effective_render_px,
        fuzzy_matches_item, is_supported_reference_target, set_textarea_text, textarea_from_text,
        textarea_lines, textarea_text,
    };
    use crate::action::Action;
    use crate::event::map_key;
    use crate::graphics::{Graphics, TerminalCellSize};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use nullspace_core::{Equation, EquationId, EquationSummary, Quantity, Variable};
    use ratatui::layout::Size;
    use std::path::PathBuf;

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
    fn editor_field_all_matches_discriminants() {
        for (index, field) in EditorField::ALL.iter().enumerate() {
            assert_eq!(field.index(), index);
        }
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
    fn command_matching_filters_by_prefix() {
        assert_eq!(
            command_matches(""),
            [
                "delete",
                "exit",
                "new",
                "openReference",
                "quantities",
                "search",
                "tags",
                "trash"
            ]
        );
        assert_eq!(command_matches("o"), ["openReference"]);
        assert_eq!(command_matches("q"), ["quantities"]);
        assert_eq!(command_matches("s"), ["search"]);
        assert_eq!(command_matches("t"), ["tags", "trash"]);
        assert_eq!(command_matches("D"), ["delete"]);
    }

    #[test]
    fn reference_open_targets_allow_https_and_local_files() {
        assert!(is_supported_reference_target("https://example.test"));
        assert!(is_supported_reference_target("/tmp/paper.pdf"));
        assert!(is_supported_reference_target("paper.pdf"));
        assert!(is_supported_reference_target("file:///tmp/paper.pdf"));
        assert!(!is_supported_reference_target("http://example.test"));
        assert!(!is_supported_reference_target(
            "ftp://example.test/file.pdf"
        ));
    }

    #[test]
    fn cmdline_accept_completes_active_match() {
        let mut app = test_app();
        app.mode = super::Mode::Cmdline;
        app.cmdline = Some(CmdlineState {
            input: "se".to_string(),
            cursor: 2,
            selected: 0,
        });

        app.accept_cmdline();

        let cmdline = app.cmdline.expect("cmdline should remain open");
        assert_eq!(cmdline.input, "search");
        assert_eq!(cmdline.cursor, "search".len());
    }

    #[test]
    fn cmdline_selection_cycles_through_matches() {
        let mut app = test_app_with_cmdline("");

        app.input_cmdline(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ));
        app.execute_cmdline();

        assert!(app.should_quit);
    }

    #[test]
    fn execute_cmdline_exit_quits() {
        let mut app = test_app_with_cmdline("exit");

        app.execute_cmdline();

        assert!(app.should_quit);
        assert!(app.cmdline.is_none());
    }

    #[test]
    fn execute_cmdline_new_opens_editor() {
        let mut app = test_app_with_cmdline("new");

        app.execute_cmdline();

        assert!(matches!(app.mode, super::Mode::Editor));
        assert!(app.editor.is_some());
        assert!(app.cmdline.is_none());
    }

    #[test]
    fn execute_cmdline_delete_opens_confirm_delete() {
        let mut app = test_app_with_cmdline("delete");

        app.execute_cmdline();

        assert!(matches!(app.mode, super::Mode::ConfirmDelete(_)));
        assert!(app.cmdline.is_none());
    }

    #[test]
    fn execute_cmdline_search_enters_search() {
        let mut app = test_app_with_cmdline("search");

        app.execute_cmdline();

        assert!(matches!(app.mode, super::Mode::Search));
        assert!(app.cmdline.is_none());
    }

    #[test]
    fn execute_cmdline_trash_opens_trash() {
        let mut app = test_app_with_cmdline("trash");

        app.execute_cmdline();

        assert!(matches!(app.mode, super::Mode::Trash));
        assert!(app.cmdline.is_none());
    }

    #[test]
    fn execute_cmdline_tags_opens_tag_picker() {
        let mut app = test_app_with_cmdline("tags");

        app.execute_cmdline();

        assert!(matches!(app.mode, super::Mode::TagPicker));
        assert!(app.cmdline.is_none());
    }

    #[test]
    fn variable_form_updates_list_display_and_saved_equation() {
        let mut app = test_app();
        app.open_editor(None);
        {
            let editor = app.editor.as_mut().unwrap();
            editor.set_field_text(EditorField::Name, "Velocity".to_string());
            editor.set_field_text(EditorField::Latex, "v = x/t".to_string());
            editor.focus = EditorField::Variables;
        }

        app.open_variable_form(None);
        {
            let editor = app.editor.as_mut().unwrap();
            set_textarea_text(
                editor.variable_form.fields.get_mut(0).unwrap(),
                "v".to_string(),
            );
            set_textarea_text(
                editor.variable_form.fields.get_mut(1).unwrap(),
                "velocity".to_string(),
            );
        }
        app.save_variable_form();

        let expected_variables = {
            let editor = app.editor.as_ref().unwrap();
            assert!(matches!(app.mode, super::Mode::Editor));
            assert_eq!(editor.variables.len(), 1);
            assert_eq!(editor.variables[0].symbol, "v");
            assert_eq!(editor.variables[0].description, "velocity");
            assert_eq!(editor.field_text(EditorField::Variables), "v = velocity");
            editor.variables.clone()
        };

        app.save_editor().unwrap();
        let saved_id = app.editor.as_ref().unwrap().editing.unwrap();
        let saved = app.store.get(saved_id).unwrap();
        assert_eq!(saved.variables, expected_variables);
    }

    #[test]
    fn tag_picker_rows_sorted_alphabetically_with_untagged_first() {
        let mut app = test_app();
        app.untagged_count = 2;
        app.tag_counts = vec![
            ("polaron".to_string(), 3),
            ("DFT".to_string(), 5),
            ("diagmc".to_string(), 7),
        ];

        assert_eq!(
            app.tag_picker_rows(),
            vec![
                super::TagPickerRow::Untagged { count: 2 },
                super::TagPickerRow::Tag {
                    name: "DFT".to_string(),
                    count: 5,
                },
                super::TagPickerRow::Tag {
                    name: "diagmc".to_string(),
                    count: 7,
                },
                super::TagPickerRow::Tag {
                    name: "polaron".to_string(),
                    count: 3,
                },
            ]
        );
    }

    #[test]
    fn tag_picker_rows_omits_untagged_when_zero() {
        let mut app = test_app();
        app.untagged_count = 0;
        app.tag_counts = vec![("dft".to_string(), 1)];

        assert_eq!(
            app.tag_picker_rows(),
            vec![super::TagPickerRow::Tag {
                name: "dft".to_string(),
                count: 1,
            }]
        );
    }

    #[test]
    fn tag_picker_apply_sets_exact_tag_filter() {
        let mut app = test_app();
        insert_test_equation(&mut app, "Tagged exact", "tagged_exact = 1", &["dft"]);
        insert_test_equation(
            &mut app,
            "Tagged substring",
            "tagged_substring = 1",
            &["dft-plus-u"],
        );
        app.reload().unwrap();
        app.apply(Action::OpenTags);
        let rows = app.tag_picker_rows();
        app.tag_picker_cursor = rows
            .iter()
            .position(|row| {
                matches!(
                    row,
                    super::TagPickerRow::Tag { name, .. } if name == "dft"
                )
            })
            .unwrap();

        app.apply(Action::TagPickerApply);

        assert_eq!(
            app.browser_filter,
            super::BrowserFilter::Tag("dft".to_string())
        );
        assert!(app.items.iter().any(|item| item.name == "Tagged exact"));
        assert!(!app.items.iter().any(|item| item.name == "Tagged substring"));
    }

    #[test]
    fn tag_picker_apply_untagged_row_sets_untagged_filter() {
        let mut app = test_app();
        insert_test_equation(&mut app, "App untagged", "app_untagged = 1", &[]);
        app.reload().unwrap();
        app.apply(Action::OpenTags);
        app.tag_picker_cursor = 0;

        app.apply(Action::TagPickerApply);

        assert_eq!(app.browser_filter, super::BrowserFilter::Untagged);
        assert!(app.items.iter().any(|item| item.name == "App untagged"));
        assert_eq!(app.items.len(), app.store.untagged().unwrap().len());
    }

    #[test]
    fn tag_picker_cancel_leaves_filter_untouched() {
        let mut app = test_app();
        app.browser_filter = super::BrowserFilter::Search("energy".to_string());

        app.apply(Action::OpenTags);
        app.apply(Action::TagPickerCancel);

        assert_eq!(
            app.browser_filter,
            super::BrowserFilter::Search("energy".to_string())
        );
        assert!(matches!(app.mode, super::Mode::Browser));
    }

    #[test]
    fn auto_link_classifies_variables() {
        let mut app = test_app();
        let e = Quantity::new("E".to_string());
        let mut g1 = Quantity::new("G".to_string());
        g1.name = "full Green's function".to_string();
        let mut g2 = Quantity::new("G".to_string());
        g2.name = "imaginary-time Green's function".to_string();
        app.store.insert_quantity(&e).unwrap();
        app.store.insert_quantity(&g1).unwrap();
        app.store.insert_quantity(&g2).unwrap();
        app.reload().unwrap();
        app.open_editor(None);
        {
            let editor = app.editor.as_mut().unwrap();
            editor.focus = EditorField::Variables;
            editor.variables = vec![
                Variable {
                    symbol: "E".to_string(),
                    description: "energy".to_string(),
                    quantity_id: None,
                },
                Variable {
                    symbol: "G".to_string(),
                    description: "Green's function".to_string(),
                    quantity_id: None,
                },
                Variable {
                    symbol: "x".to_string(),
                    description: "position".to_string(),
                    quantity_id: None,
                },
            ];
        }

        app.link_variables_to_quantities().unwrap();

        let editor = app.editor.as_ref().unwrap();
        assert_eq!(editor.variables[0].quantity_id, Some(e.id));
        assert!(editor.variables[2].quantity_id.is_some());
        assert_eq!(
            app.store.quantities_by_symbol("x").unwrap()[0].description,
            "position"
        );
        assert!(matches!(app.mode, super::Mode::QuantityResolver));
        assert_eq!(app.quantity_resolver.as_ref().unwrap().queue, vec![1]);
    }

    #[test]
    fn resolver_accept_links_candidate() {
        let mut app = test_app();
        let first = Quantity::new("G".to_string());
        let second = Quantity::new("G".to_string());
        app.store.insert_quantity(&first).unwrap();
        app.store.insert_quantity(&second).unwrap();
        app.reload().unwrap();
        app.open_editor(None);
        {
            let editor = app.editor.as_mut().unwrap();
            editor.focus = EditorField::Variables;
            editor.variables = vec![Variable {
                symbol: "G".to_string(),
                description: "Green's function".to_string(),
                quantity_id: None,
            }];
        }
        app.link_variables_to_quantities().unwrap();

        app.apply(Action::ResolverAccept);

        let editor = app.editor.as_ref().unwrap();
        assert!(
            [first.id, second.id]
                .into_iter()
                .any(|id| editor.variables[0].quantity_id == Some(id))
        );
        assert!(editor.dirty);
        assert!(matches!(app.mode, super::Mode::Editor));
    }

    #[test]
    fn resolver_skip_is_counted_in_status() {
        let mut app = test_app();
        let energy = Quantity::new("E".to_string());
        app.store.insert_quantity(&energy).unwrap();
        app.store
            .insert_quantity(&Quantity::new("G".to_string()))
            .unwrap();
        app.store
            .insert_quantity(&Quantity::new("G".to_string()))
            .unwrap();
        app.reload().unwrap();
        app.open_editor(None);
        {
            let editor = app.editor.as_mut().unwrap();
            editor.focus = EditorField::Variables;
            editor.variables = vec![
                Variable {
                    symbol: "E".to_string(),
                    description: "energy".to_string(),
                    quantity_id: None,
                },
                Variable {
                    symbol: "G".to_string(),
                    description: "Green's function".to_string(),
                    quantity_id: None,
                },
            ];
        }
        app.link_variables_to_quantities().unwrap();

        app.apply(Action::ResolverSkip);

        assert!(matches!(app.mode, super::Mode::Editor));
        assert_eq!(
            app.status,
            "Variables linked: 1 linked, 0 created, 1 skipped"
        );
        let editor = app.editor.as_ref().unwrap();
        assert_eq!(editor.variables[0].quantity_id, Some(energy.id));
        assert_eq!(editor.variables[1].quantity_id, None);
    }

    #[test]
    fn unlink_clears_quantity_id() {
        let mut app = test_app();
        let quantity = Quantity::new("E".to_string());
        app.open_editor(None);
        {
            let editor = app.editor.as_mut().unwrap();
            editor.focus = EditorField::Variables;
            editor.variables = vec![Variable {
                symbol: "E".to_string(),
                description: "energy".to_string(),
                quantity_id: Some(quantity.id),
            }];
        }

        app.input_editor(key('u'));

        let editor = app.editor.as_ref().unwrap();
        assert_eq!(editor.variables[0].quantity_id, None);
        assert!(editor.dirty);
    }

    #[test]
    fn browser_title_reflects_tag_and_untagged_filters() {
        let mut app = test_app();

        app.browser_filter = super::BrowserFilter::Tag("dft".to_string());
        assert_eq!(app.browser_title(), "Tag: dft");

        app.browser_filter = super::BrowserFilter::Untagged;
        assert_eq!(app.browser_title(), "Untagged");
    }

    #[test]
    fn execute_cmdline_unknown_returns_to_browser_with_status() {
        let mut app = test_app_with_cmdline("wat");

        app.execute_cmdline();

        assert!(matches!(app.mode, super::Mode::Browser));
        assert_eq!(app.status, "Unknown command: wat");
        assert!(app.cmdline.is_none());
    }

    #[test]
    fn gg_prefix_maps_to_browser_top() {
        let mut app = test_app();
        app.cursor = app.items.len().saturating_sub(1);
        app.list_scroll_offset = app.cursor;

        let first_g = map_key(key('g'), &app);
        assert!(matches!(first_g, Action::StartGoPrefix));
        app.apply(first_g);
        assert!(app.vim_go_prefix);

        let second_g = map_key(key('g'), &app);
        assert!(matches!(second_g, Action::MoveToTop));
        app.apply(second_g);

        assert_eq!(app.cursor, 0);
        assert_eq!(app.list_scroll_offset, 0);
        assert!(!app.vim_go_prefix);
    }

    #[test]
    fn shift_g_moves_browser_to_bottom() {
        let mut app = test_app();
        app.list_visible_height = 5;

        let action = map_key(key('G'), &app);
        app.apply(action);

        assert_eq!(app.cursor, app.items.len() - 1);
        assert_eq!(app.list_scroll_offset, app.items.len().saturating_sub(2));
    }

    #[test]
    fn non_prefix_action_clears_gg_prefix() {
        let mut app = test_app();

        app.apply(Action::StartGoPrefix);
        app.apply(Action::None);

        assert!(!app.vim_go_prefix);
    }

    #[test]
    fn question_mark_opens_and_closes_help() {
        let mut app = test_app();

        let open = map_key(key('?'), &app);
        assert!(matches!(open, Action::OpenHelp));
        app.apply(open);
        assert!(app.help_open);

        let close = map_key(key('?'), &app);
        assert!(matches!(close, Action::CloseHelp));
        app.apply(close);
        assert!(!app.help_open);
    }

    #[test]
    fn help_modal_consumes_other_keys() {
        let mut app = test_app();
        app.help_open = true;

        assert!(matches!(map_key(key('j'), &app), Action::None));
        assert!(matches!(map_key(key('q'), &app), Action::None));
    }

    #[test]
    fn effective_render_px_caps_to_preview_height() {
        let size = Some(Size {
            width: 80,
            height: 5,
        });
        let cell_size = TerminalCellSize {
            width: 10,
            height: 20,
        };

        assert_eq!(effective_render_px(512, size, cell_size), 100);
    }

    #[test]
    fn effective_render_px_does_not_upscale_small_equations() {
        let size = Some(Size {
            width: 80,
            height: 20,
        });
        let cell_size = TerminalCellSize {
            width: 10,
            height: 20,
        };

        assert_eq!(effective_render_px(48, size, cell_size), 48);
    }

    #[test]
    fn effective_render_px_uses_detected_cell_height() {
        let size = Some(Size {
            width: 80,
            height: 5,
        });
        let cell_size = TerminalCellSize {
            width: 9,
            height: 18,
        };

        assert_eq!(effective_render_px(512, size, cell_size), 90);
    }

    #[test]
    fn effective_render_px_uses_full_detected_cell_box() {
        let size = Some(Size {
            width: 80,
            height: 5,
        });
        let cell_size = TerminalCellSize {
            width: 12,
            height: 26,
        };

        assert_eq!(effective_render_px(512, size, cell_size), 130);
    }

    #[test]
    fn default_equation_px_is_five_cell_heights() {
        let cell_size = TerminalCellSize {
            width: 10,
            height: 20,
        };

        assert_eq!(default_equation_px(cell_size), 100);
    }

    fn test_app_with_cmdline(input: &str) -> super::AppState {
        let mut app = test_app();
        app.mode = super::Mode::Cmdline;
        app.cmdline = Some(CmdlineState {
            input: input.to_string(),
            cursor: input.len(),
            selected: 0,
        });
        app
    }

    fn test_app() -> super::AppState {
        let path = test_db_path();
        let _ = std::fs::remove_file(&path);
        super::AppState::open(
            &path,
            Graphics::test(TerminalCellSize {
                width: 10,
                height: 20,
            }),
        )
        .expect("test app should open")
    }

    fn insert_test_equation(app: &mut super::AppState, name: &str, latex: &str, tags: &[&str]) {
        let mut eq = Equation::new(name.to_string(), latex.to_string());
        eq.tags = tags.iter().map(|tag| tag.to_string()).collect();
        app.store.insert(&eq).unwrap();
    }

    fn test_db_path() -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "nullspace-cmdline-test-{}-{:?}.sqlite3",
            std::process::id(),
            std::thread::current().id()
        ));
        path
    }

    fn key(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)
    }
}
