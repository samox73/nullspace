use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use image::RgbaImage;
use nullspace_core::{
    Equation, EquationId, EquationSummary, Quantity, QuantityId, Reference, Store, TrashEntry,
    Variable,
};
use ratatui::layout::Size;
use ratatui_image::protocol::StatefulProtocol;
use tui_textarea::TextArea;

use crate::graphics::{Graphics, TerminalCellSize};
use crate::protocol_warm_worker::{ProtocolWarmResult, ProtocolWarmWorker};
use crate::render_queue::{QueueResult, RenderQueue};

use super::text::{set_textarea_text, textarea_from_text, textarea_text};

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
    Scan,
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
    pub active: bool,
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

    pub(super) fn field_texts(&self) -> [String; EditorField::COUNT] {
        EditorField::ALL.map(|field| self.field_text(field))
    }

    pub(super) fn set_field_text(&mut self, field: EditorField, text: String) {
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
    pub(super) fn empty() -> Self {
        Self {
            fields: std::array::from_fn(|_| textarea_from_text("")),
            focus: 0,
            editing: None,
            error: None,
        }
    }

    pub(super) fn from_variable(variable: &Variable, index: usize) -> Self {
        let values = [variable.symbol.clone(), variable.description.clone()];
        Self {
            fields: values.each_ref().map(|value| textarea_from_text(value)),
            focus: 0,
            editing: Some(index),
            error: None,
        }
    }

    pub(super) fn text_at(&self, index: usize) -> String {
        textarea_text(&self.fields[index])
    }
}

impl ReferenceForm {
    pub(super) fn empty() -> Self {
        Self {
            fields: std::array::from_fn(|_| textarea_from_text("")),
            focus: 0,
            editing: None,
            error: None,
        }
    }

    pub(super) fn from_reference(reference: &Reference, index: usize) -> Self {
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

    pub(super) fn text_at(&self, index: usize) -> String {
        textarea_text(&self.fields[index])
    }
}

impl QuantityFormState {
    pub(super) fn empty() -> Self {
        Self {
            fields: std::array::from_fn(|_| textarea_from_text("")),
            focus: 0,
            editing: None,
            error: None,
        }
    }

    pub(super) fn from_quantity(quantity: &Quantity) -> Self {
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

    pub(super) fn text_at(&self, index: usize) -> String {
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
    pub return_mode: Mode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagPickerRow {
    Untagged { count: usize },
    Tag { name: String, count: usize },
}

#[derive(Clone)]
pub(super) struct NavSnapshot {
    pub(super) mode: Mode,
    pub(super) browser_filter: BrowserFilter,
    pub(super) cursor: usize,
    pub(super) list_scroll_offset: usize,
    pub(super) selected_id: Option<EquationId>,
    pub(super) editor: Option<EditorState>,
    pub(super) quantity_cursor: usize,
    pub(super) quantity_scroll_offset: usize,
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
    pub scan: Option<super::scan::ScanState>,
    pub scan_review: bool,
    pub scan_agent: super::scan::ScanAgent,
    pub(super) nav_stack: Vec<NavSnapshot>,
    pub(super) render_queue: RenderQueue,
    pub(super) protocol_warm_worker: ProtocolWarmWorker,
    pub(super) pending_protocol_results: VecDeque<ProtocolWarmResult>,
    pub(super) pending_queue_results: VecDeque<QueueResult>,
    pub(super) generation: u64,
    pub(super) dispatched_generation: u64,
    pub(super) last_change: Instant,
    pub(super) cache: HashMap<u64, RgbaImage>,
    pub(super) cache_order: VecDeque<u64>,
    pub(super) queued_keys: HashSet<u64>,
    pub(super) render_failed: HashSet<u64>,
    pub(super) needs_warm_submit: bool,
    pub(super) protocol_warm_inflight: HashMap<u64, Size>,
    pub(super) protocol_cache: HashMap<u64, StatefulProtocol>,
    pub(super) protocol_cache_order: VecDeque<u64>,
    pub(super) protocol_cache_epoch: u64,
    pub(super) preview_cache_key: u64,
    pub(super) preview_warm_size: Option<Size>,
    pub(super) graphics: Graphics,
}

pub struct Notification {
    pub message: String,
    pub created_at: Instant,
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
