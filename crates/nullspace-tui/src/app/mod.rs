mod actions;
mod cmdline;
mod editor;
mod render;
mod scan;
mod state;
mod text;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use image::RgbaImage;
use nullspace_core::reference::{normalize_doi, normalize_pages, reference_link};
use nullspace_core::{
    Equation, EquationId, EquationSummary, Quantity, QuantityId, Reference, Store, Variable,
    render::validate_latex,
};
use ratatui::layout::Size;
use ratatui_image::protocol::StatefulProtocol;

pub use cmdline::command_matches;
#[cfg(test)]
use editor::fuzzy_matches_item;
pub use editor::quantity_label;
use render::default_equation_px;
#[cfg(test)]
use render::effective_render_px;
pub use scan::{ScanAgent, ScanPhase};
pub use state::{
    AppState, BrowserFilter, BrowserFilterFocus, CacheStatus, CmdlineState, EditorField,
    EditorState, LayoutOrientation, Mode, Pane, QUANTITY_FIELD_LABELS, REFERENCE_FIELD_LABELS,
    RelatedPickerFocus, ResolverRow, TagPickerRow, VARIABLE_FIELD_LABELS,
};
use state::{
    NavSnapshot, Notification, QuantityFormState, QuantityResolverState, ReferenceForm,
    RelatedPickerState, VariableForm,
};
use text::textarea_from_text;

use crate::graphics::{Graphics, TerminalCellSize};
use crate::protocol_warm_worker::{
    ProtocolWarmJob, ProtocolWarmOutcome, ProtocolWarmResult, ProtocolWarmSource,
    ProtocolWarmWorker,
};
use crate::render_cache;
use crate::render_queue::{QueueJob, QueueResult, RenderQueue};

use cmdline::{
    accept_cmdline_state, command_action, cycle_cmdline_selection, exact_command, selected_command,
};

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
            scan: None,
            scan_review: false,
            scan_agent: ScanAgent::Claude,
            nav_stack: Vec::new(),
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

    fn push_nav(&mut self) {
        self.nav_stack.push(NavSnapshot {
            mode: self.mode,
            browser_filter: self.browser_filter.clone(),
            cursor: self.cursor,
            list_scroll_offset: self.list_scroll_offset,
            selected_id: if matches!(self.mode, Mode::Editor) {
                self.editor
                    .as_ref()
                    .and_then(|editor| editor.editing)
                    .or_else(|| self.selected.as_ref().map(|equation| equation.id))
            } else {
                self.selected_id()
                    .or_else(|| self.selected.as_ref().map(|equation| equation.id))
            },
            editor: self.editor.clone(),
            quantity_cursor: self.quantity_cursor,
            quantity_scroll_offset: self.quantity_scroll_offset,
        });
    }

    fn clear_nav(&mut self) {
        self.nav_stack.clear();
    }

    fn restore_nav(&mut self) -> anyhow::Result<bool> {
        let Some(snapshot) = self.nav_stack.pop() else {
            return Ok(false);
        };
        self.mode = snapshot.mode;
        self.browser_filter = snapshot.browser_filter;
        self.cursor = snapshot.cursor;
        self.list_scroll_offset = snapshot.list_scroll_offset;
        self.quantity_cursor = snapshot.quantity_cursor;
        self.quantity_scroll_offset = snapshot.quantity_scroll_offset;
        self.editor = snapshot.editor;
        self.refresh_items()?;
        if let Some(id) = snapshot.selected_id
            && let Some(index) = self.items.iter().position(|item| item.id == id)
        {
            self.cursor = index;
        }
        if self.cursor >= self.items.len() {
            self.cursor = self.items.len().saturating_sub(1);
        }
        self.list_scroll_offset = self.list_scroll_offset.min(self.cursor);
        self.selected = snapshot
            .selected_id
            .and_then(|id| self.store.get(id).ok())
            .or_else(|| self.selected_id().and_then(|id| self.store.get(id).ok()));
        self.schedule_selected();
        Ok(true)
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
        let return_mode = self
            .cmdline
            .as_ref()
            .map(|cmdline| cmdline.return_mode)
            .unwrap_or(Mode::Browser);
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
        self.mode = return_mode;
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
mod tests;
