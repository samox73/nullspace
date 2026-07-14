use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect, Size},
    prelude::Position,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, Wrap,
    },
};
use ratatui_image::{Resize, ResizeEncodeRender, StatefulImage};

use nullspace_core::{EquationSummary, TrashEntry};

use crate::app::{AppState, QUANTITY_FIELD_LABELS, ScanPhase, TagPickerRow, command_matches};

const CMDLINE_WIDTH: u16 = 60;
const CMDLINE_PROMPT_HEIGHT: u16 = 3;
const CMDLINE_LIST_MAX_HEIGHT: u16 = 8;
const CMDLINE_TOP_OFFSET: u16 = 2;

#[derive(Debug, Clone)]
pub struct EquationListRow {
    pub marker: String,
    pub name: String,
    pub unicode_approx: String,
}

impl EquationListRow {
    pub fn new(marker: impl Into<String>, item: &EquationSummary) -> Self {
        Self {
            marker: marker.into(),
            name: item.name.clone(),
            unicode_approx: item.unicode_approx.clone(),
        }
    }
}

pub fn equation_list(
    rows: &[EquationListRow],
    selected: Option<usize>,
    offset: usize,
    title: impl Into<Line<'static>>,
    focused: bool,
) -> (List<'static>, ListState) {
    equation_list_inner(rows, selected, offset, title, focused, None)
}

pub fn equation_list_with_empty_message(
    rows: &[EquationListRow],
    selected: Option<usize>,
    offset: usize,
    title: impl Into<Line<'static>>,
    focused: bool,
    empty_message: &'static str,
) -> (List<'static>, ListState) {
    equation_list_inner(rows, selected, offset, title, focused, Some(empty_message))
}

fn equation_list_inner(
    rows: &[EquationListRow],
    selected: Option<usize>,
    offset: usize,
    title: impl Into<Line<'static>>,
    focused: bool,
    empty_message: Option<&'static str>,
) -> (List<'static>, ListState) {
    let mut items = rows
        .iter()
        .enumerate()
        .flat_map(|(index, row)| {
            let item = ListItem::new(vec![
                Line::from(vec![
                    Span::styled(row.marker.clone(), Style::default().fg(Color::Yellow)),
                    Span::raw(" "),
                    Span::styled(
                        row.name.clone(),
                        Style::default().add_modifier(ratatui::style::Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![Span::raw("  "), Span::raw(row.unicode_approx.clone())]),
            ]);
            let spacer = (index + 1 < rows.len()).then(|| ListItem::new(Line::from("")));
            std::iter::once(item).chain(spacer)
        })
        .collect::<Vec<_>>();
    if items.is_empty()
        && let Some(message) = empty_message
    {
        items.push(ListItem::new(Line::from(message)));
    }
    let mut state = ListState::default().with_offset(offset * 2);
    if !rows.is_empty() {
        state.select(selected.map(|index| index * 2));
    }
    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(if focused {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                }),
        )
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .highlight_symbol("> ");
    (list, state)
}

pub struct SearchBox<'a> {
    pub title: &'a str,
    pub label: &'a str,
    pub query: &'a str,
    pub cursor: usize,
    pub hint: &'a str,
    pub details: Vec<String>,
    pub focused: bool,
}

pub fn search_box(frame: &mut Frame<'_>, area: Rect, props: SearchBox<'_>) {
    let block = Block::default()
        .title(props.title)
        .borders(Borders::ALL)
        .border_style(if props.focused {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        });
    let inner = block.inner(area);
    let label_width = props.label.chars().count() as u16;
    let input_width = inner.width.saturating_sub(label_width).max(1) as usize;
    let (visible_query, cursor_column) =
        visible_search_input(props.query, props.cursor, input_width);
    let mut lines = vec![Line::from(vec![
        Span::raw(props.label.to_string()),
        Span::raw(visible_query),
    ])];
    if !props.hint.is_empty() {
        lines.push(Line::styled(
            props.hint.to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }
    lines.extend(
        props
            .details
            .into_iter()
            .map(|line| Line::styled(line, Style::default().fg(Color::DarkGray))),
    );

    frame.render_widget(Paragraph::new(lines).block(block), area);

    if props.focused && inner.width > label_width && inner.height > 0 {
        frame.set_cursor_position(Position::new(
            inner.x + label_width + cursor_column.min(input_width) as u16,
            inner.y,
        ));
    }
}

pub fn cmdline(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let Some(cmdline) = &app.cmdline else {
        return;
    };

    let matches = command_matches(&cmdline.input);
    let selected = cmdline.selected.min(matches.len().saturating_sub(1));
    let ghost = matches
        .get(selected)
        .and_then(|command| command_ghost(command, &cmdline.input))
        .unwrap_or("");
    let prompt_area = cmdline_prompt_area(area);
    let block = Block::default().title("Cmdline").borders(Borders::ALL);
    let inner = block.inner(prompt_area);
    let cursor_column = 2 + cmdline.input[..cmdline.cursor].chars().count() as u16;

    frame.render_widget(Clear, prompt_area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("> "),
            Span::raw(cmdline.input.clone()),
            Span::styled(ghost.to_string(), Style::default().fg(Color::DarkGray)),
        ]))
        .block(block),
        prompt_area,
    );
    if inner.height > 0 {
        frame.set_cursor_position(Position::new(
            inner.x + cursor_column.min(inner.width.saturating_sub(1)),
            inner.y,
        ));
    }

    if matches.is_empty() {
        return;
    }
    let list_height = (matches.len() as u16 + 2).min(CMDLINE_LIST_MAX_HEIGHT);
    let list_area = cmdline_list_area(area, list_height);
    let items = matches
        .into_iter()
        .map(|command| ListItem::new(Line::from(command)))
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(Block::default().title("Commands").borders(Borders::ALL))
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White));
    let mut state = ListState::default().with_selected(Some(selected));

    frame.render_widget(Clear, list_area);
    frame.render_stateful_widget(list, list_area, &mut state);
}

pub fn clear_cmdline_overlay(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(Clear, cmdline_prompt_area(area));
    frame.render_widget(Clear, cmdline_list_area(area, CMDLINE_LIST_MAX_HEIGHT));
}

fn command_ghost<'a>(command: &'a str, input: &str) -> Option<&'a str> {
    if input.len() < command.len() && command[..input.len()].eq_ignore_ascii_case(input) {
        Some(&command[input.len()..])
    } else {
        None
    }
}

fn visible_search_input(query: &str, cursor: usize, width: usize) -> (String, usize) {
    if width == 0 {
        return (String::new(), 0);
    }
    let cursor = cursor.min(query.len());
    let cursor_chars = query
        .char_indices()
        .take_while(|(index, _)| *index < cursor)
        .count();
    let start_chars = cursor_chars.saturating_sub(width.saturating_sub(1));
    let visible = query.chars().skip(start_chars).take(width).collect();
    (visible, cursor_chars - start_chars)
}

pub fn preview_pane(frame: &mut Frame<'_>, area: Rect, app: &mut AppState, title: &str) {
    let block = Block::default().title(title).borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // A hard render error with no image to fall back on: show the error full-pane.
    if app.preview_protocol.is_none() && app.preview_error.is_some() {
        if let Some(error) = &app.preview_error {
            frame.render_widget(render_error(error), inner);
        }
        return;
    }

    let error_height = if app.preview_error.is_some() {
        4.min(inner.height)
    } else {
        0
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(error_height)])
        .split(inner);
    let image_size = Size {
        width: chunks[0].width,
        height: chunks[0].height,
    };
    app.set_preview_warm_size(image_size);

    // Only ever render a protocol that is already encoded for this exact area. Encoding on
    // the UI thread blocks scrolling, so anything that still needs (re)encoding is handed
    // to the background encoder and a spinner is shown until the result is promoted in.
    let ready = app.preview_protocol.as_ref().is_some_and(|protocol| {
        protocol
            .needs_resize(&Resize::Fit(None), image_size)
            .is_none()
    });

    if ready {
        frame.render_widget(Clear, chunks[0]);
        if let Some(protocol) = &mut app.preview_protocol {
            let image_area = centered_image_area(protocol, chunks[0]);
            frame.render_stateful_widget(StatefulImage::default(), image_area, protocol);
        }
    } else {
        if app.preview_protocol.is_some() {
            app.request_preview_encode(image_size);
        }
        let spinner = app.cache_spinner();
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(0),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(chunks[0]);
        frame.render_widget(
            Paragraph::new(spinner).alignment(Alignment::Center),
            rows[1],
        );
    }

    if let Some(error) = &app.preview_error {
        frame.render_widget(render_stale_warning(error), chunks[1]);
    }
}

pub fn message_pane(frame: &mut Frame<'_>, area: Rect, title: &str, message: &'static str) {
    let block = Block::default().title(title).borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(message).alignment(Alignment::Center),
        centered_text_area(inner),
    );
}

pub fn scan_screen(frame: &mut Frame<'_>, app: &AppState) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());
    let Some(scan) = &app.scan else {
        message_pane(frame, outer[0], "Scan", "No scan active");
        status_bar(frame, outer[1], app);
        return;
    };
    match scan.phase {
        ScanPhase::AwaitingPaste => {
            let block = Block::default().title("Scan").borders(Borders::ALL);
            let inner = block.inner(outer[0]);
            frame.render_widget(block, outer[0]);
            let lines = vec![
                Line::from("scan equation from image"),
                Line::from(scan.settings_label()),
                Line::from("m: model | i: intelligence"),
                Line::from("copy an equation image, then press p to paste"),
                Line::from("Esc to leave"),
            ];
            let area = centered_lines_area(inner, lines.len() as u16);
            frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
        }
        ScanPhase::Running => {
            let title = format!("Scanning... {}", app.cache_spinner());
            scan_logs(frame, outer[0], &title, &scan.logs, None);
        }
        ScanPhase::Failed => {
            scan_logs(
                frame,
                outer[0],
                "Scan failed",
                &scan.logs,
                Some("p: paste again | :rescan: retry same image | Esc: leave"),
            );
        }
    }
    status_bar(frame, outer[1], app);
}

fn scan_logs(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    logs: &[String],
    hint: Option<&'static str>,
) {
    let block = Block::default().title(title).borders(Borders::ALL);
    let inner = block.inner(area);
    let hint_rows = hint.is_some() as u16;
    let log_height = inner.height.saturating_sub(hint_rows) as usize;
    let lines = visible_scan_log_lines(logs, log_height)
        .into_iter()
        .map(Line::from)
        .collect::<Vec<_>>();
    frame.render_widget(block, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(hint_rows)])
        .split(inner);
    frame.render_widget(Paragraph::new(lines), chunks[0]);
    if let Some(hint) = hint {
        frame.render_widget(
            Paragraph::new(hint).style(Style::default().fg(Color::Yellow)),
            chunks[1],
        );
    }
}

fn visible_scan_log_lines(logs: &[String], height: usize) -> Vec<String> {
    let rows = logs
        .iter()
        .flat_map(|log| {
            let rows = log.lines().collect::<Vec<_>>();
            if rows.is_empty() { vec![""] } else { rows }
        })
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    rows[rows.len().saturating_sub(height)..].to_vec()
}

fn centered_lines_area(area: Rect, height: u16) -> Rect {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(height.min(area.height)),
            Constraint::Min(0),
        ])
        .split(area);
    rows[1]
}

fn centered_text_area(area: Rect) -> Rect {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);
    rows[1]
}

fn render_stale_warning(error: &str) -> Paragraph<'_> {
    Paragraph::new(vec![
        Line::styled(
            "Warning: showing last successfully rendered equation.",
            Style::default().fg(Color::Yellow),
        ),
        Line::styled(
            format!("Render error: {error}"),
            Style::default().fg(Color::Red),
        ),
    ])
    .wrap(Wrap { trim: false })
}

fn render_error(error: &str) -> Paragraph<'_> {
    Paragraph::new(Line::styled(
        format!("Render error: {error}"),
        Style::default().fg(Color::Red),
    ))
    .wrap(Wrap { trim: false })
}

fn centered_image_area(protocol: &ratatui_image::protocol::StatefulProtocol, area: Rect) -> Rect {
    let rendered = protocol.size_for(
        Resize::Fit(None),
        Size {
            width: area.width,
            height: area.height,
        },
    );
    let width = rendered.width.min(area.width);
    let height = rendered.height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

pub fn status_bar(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let help = "q quit  ? help";
    let graphics = if app.graphics_ok {
        " | terminal graphics detected"
    } else {
        " | no terminal graphics detected"
    };
    let line = Line::from(vec![
        Span::styled(help, Style::default().fg(Color::Yellow)),
        Span::raw(" | "),
        Span::raw(&app.status),
        Span::styled(graphics, Style::default().fg(Color::Yellow)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

pub fn help_modal(frame: &mut Frame<'_>) {
    let rows = help_rows().into_iter().map(|(context, key, action)| {
        Row::new([Cell::from(context), Cell::from(key), Cell::from(action)])
    });
    let area = centered_rect(100, 33, frame.area());
    let table = Table::new(
        rows,
        [
            Constraint::Length(18),
            Constraint::Length(18),
            Constraint::Min(24),
        ],
    )
    .header(
        Row::new(["Context", "Key", "Action"])
            .style(Style::default().fg(Color::Yellow))
            .bottom_margin(1),
    )
    .block(Block::default().title("Keybinds").borders(Borders::ALL))
    .column_spacing(2);

    frame.render_widget(Clear, area);
    frame.render_widget(table, area);
}

fn help_rows() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("Global", "?", "Open help"),
        ("Help modal", "? or Esc", "Close help"),
        ("Global", "Ctrl-C", "Quit"),
        ("Browser", "q", "Quit"),
        (
            "Browser",
            "j/k, Up/Down, gg, G",
            "Move selection; jump top/bottom",
        ),
        (
            "Browser",
            "Enter, n, c, y, d",
            "Edit, new, clone, copy LaTeX, delete",
        ),
        (
            "Browser",
            "/, :, o, Esc",
            "Search, command line, open reference, clear filter",
        ),
        (
            "Browser",
            "+/=, -, v, h/l",
            "Zoom, toggle layout, focus panes",
        ),
        (
            "Command line",
            "Type, Tab/Right, Up/Down",
            "Edit, accept completion, select command",
        ),
        ("Command line", "Enter, Esc", "Run command, cancel"),
        (
            "Search",
            "Type, Tab, Enter, Esc",
            "Edit query, switch focus, apply, clear; scopes: tag:, var:",
        ),
        (
            "Search list",
            "j/k, Up/Down, gg, G",
            "Move selection; jump top/bottom",
        ),
        (
            "Trash",
            "j/k, Up/Down, gg, G",
            "Move selection; jump top/bottom",
        ),
        (
            "Trash",
            "r, d/Delete, Esc/q",
            "Restore, permanently delete, back",
        ),
        (
            "Tags",
            "j/k, Up/Down, gg, G",
            "Move selection; jump top/bottom",
        ),
        ("Tags", "Enter, Esc/q", "Filter by tag, cancel"),
        (
            "Quantities",
            "j/k, Up/Down, gg, G",
            "Move selection; jump top/bottom",
        ),
        (
            "Quantities",
            "Enter, Esc, q, n, e, d",
            "Filter by quantity, back, quit, new, edit, delete",
        ),
        (
            "Editor",
            "j/k, Enter, Esc, Ctrl-S",
            "Move field, activate field, deactivate/back, save",
        ),
        (
            "Active editor field",
            "Arrows/Home/End",
            "Move text cursor or list selection",
        ),
        (
            "References field",
            "a, Enter, o, j/k, d",
            "Add, edit, open, move, remove reference",
        ),
        (
            "Variables field",
            "a, e, Enter, j/k, d",
            "Add, edit, open quantity, move, remove variable",
        ),
        (
            "Variables field",
            "c, u",
            "Link all to quantities, unlink one",
        ),
        (
            "Quantity resolver",
            "Type, Up/Down, Enter, Esc",
            "Filter, move, choose, skip",
        ),
        (
            "Related field",
            "r, Enter, j/k, d",
            "Choose, open, move, remove relation",
        ),
        (
            "Related picker",
            "Type, Tab, Space",
            "Search, switch focus, toggle selected",
        ),
        (
            "Related picker",
            "j/k, Up/Down, Enter, Esc",
            "Move, apply selection, cancel",
        ),
        (
            "Reference editor",
            "Tab/Shift-Tab, Enter/Ctrl-S, Esc",
            "Move fields, save, cancel",
        ),
        ("Confirm delete", "y/d/Enter, n/Esc", "Confirm or cancel"),
        ("Confirm remove", "y, n/Esc", "Remove or cancel"),
    ]
}

pub fn trash(frame: &mut Frame<'_>, app: &AppState) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());
    trash_list(frame, outer[0], &app.trash_items, app.trash_cursor);
    status_bar(frame, outer[1], app);
}

fn trash_list(frame: &mut Frame<'_>, area: Rect, entries: &[TrashEntry], cursor: usize) {
    if entries.is_empty() {
        frame.render_widget(
            Paragraph::new("Trash is empty")
                .alignment(Alignment::Center)
                .block(Block::default().title("Trash").borders(Borders::ALL)),
            area,
        );
        return;
    }

    let items = entries
        .iter()
        .map(|entry| {
            ListItem::new(vec![
                Line::from(Span::styled(
                    entry.name.clone(),
                    Style::default().add_modifier(ratatui::style::Modifier::BOLD),
                )),
                Line::from(vec![
                    Span::raw("  deleted "),
                    Span::styled(
                        entry.deleted_at.clone(),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]),
            ])
        })
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(Block::default().title("Trash").borders(Borders::ALL))
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .highlight_symbol("> ");
    let mut state = ListState::default().with_selected(Some(cursor.min(entries.len() - 1)));
    frame.render_stateful_widget(list, area, &mut state);
}

pub fn tag_picker(frame: &mut Frame<'_>, app: &mut AppState) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());
    tag_picker_list(frame, outer[0], app);
    status_bar(frame, outer[1], app);
}

pub fn quantity_picker(frame: &mut Frame<'_>, app: &mut AppState) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());
    quantity_picker_list(frame, outer[0], app);
    status_bar(frame, outer[1], app);
}

fn quantity_picker_list(frame: &mut Frame<'_>, area: Rect, app: &mut AppState) {
    app.quantity_visible_height = area.height.saturating_sub(2);
    if app.quantities.is_empty() {
        frame.render_widget(
            Paragraph::new(
                "No quantities\n\nPress n to add one, or use c in an equation's Variables field",
            )
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .title("Quantities (0/0)")
                    .borders(Borders::ALL),
            ),
            area,
        );
        return;
    }
    let items = app
        .quantities
        .iter()
        .enumerate()
        .flat_map(|(index, (quantity, count))| {
            let detail = [
                quantity.units.as_str(),
                quantity.description.lines().next().unwrap_or(""),
            ]
            .into_iter()
            .filter(|part| !part.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" - ");
            let item = ListItem::new(vec![
                Line::from(vec![
                    Span::styled(
                        crate::app::quantity_label(quantity),
                        Style::default().add_modifier(ratatui::style::Modifier::BOLD),
                    ),
                    Span::raw(format!("  ({count} eq)")),
                ]),
                Line::styled(detail, Style::default().fg(Color::DarkGray)),
            ]);
            let spacer = (index + 1 < app.quantities.len()).then(|| ListItem::new(Line::from("")));
            std::iter::once(item).chain(spacer)
        })
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(
                    "Quantities ({}/{})  (enter filter, n new, e edit, d delete)",
                    app.quantity_cursor.min(app.quantities.len() - 1) + 1,
                    app.quantities.len()
                ))
                .borders(Borders::ALL),
        )
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .highlight_symbol("> ");
    let mut state = ListState::default().with_offset(app.quantity_scroll_offset * 2);
    state.select(Some(app.quantity_cursor.min(app.quantities.len() - 1) * 2));
    frame.render_stateful_widget(list, area, &mut state);
}

pub fn quantity_form(frame: &mut Frame<'_>, app: &mut AppState) {
    let Some(form) = &mut app.quantity_form else {
        return;
    };
    let area = centered_rect(60, 16, frame.area());
    frame.render_widget(Clear, area);
    let outer = Block::default().title("Quantity").borders(Borders::ALL);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(inner);

    for index in 0..4 {
        let focused = form.focus == index;
        let block = Block::default()
            .title(QUANTITY_FIELD_LABELS[index])
            .borders(Borders::ALL)
            .border_style(if focused {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            });
        form.fields[index].set_block(block);
        form.fields[index].set_cursor_style(if focused {
            Style::default().add_modifier(ratatui::style::Modifier::REVERSED)
        } else {
            Style::default()
        });
        if index == 3 {
            form.fields[index].set_placeholder_text("SI: J; cgs: erg; a.u.: E_h");
            form.fields[index].set_placeholder_style(Style::default().fg(Color::DarkGray));
        }
        frame.render_widget(&form.fields[index], rows[index]);
    }
    let hint = match &form.error {
        Some(err) => Line::styled(err.clone(), Style::default().fg(Color::Red)),
        None => Line::styled(
            "tab next - shift-tab prev - enter save - esc cancel",
            Style::default().fg(Color::DarkGray),
        ),
    };
    frame.render_widget(Paragraph::new(hint), rows[4]);
}

fn tag_picker_list(frame: &mut Frame<'_>, area: Rect, app: &mut AppState) {
    app.tag_picker_visible_height = area.height.saturating_sub(2);
    let rows = app.tag_picker_rows();
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new("No tags")
                .alignment(Alignment::Center)
                .block(Block::default().title("Tags").borders(Borders::ALL)),
            area,
        );
        return;
    }

    let items = rows
        .iter()
        .map(|row| match row {
            TagPickerRow::Untagged { count } => ListItem::new(Line::from(vec![Span::styled(
                format!("{count:>3} (untagged)"),
                Style::default().fg(Color::DarkGray),
            )])),
            TagPickerRow::Tag { name, count } => {
                ListItem::new(Line::from(format!("{count:>3} {name}")))
            }
        })
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(Block::default().title("Tags").borders(Borders::ALL))
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .highlight_symbol("> ");
    let mut state = ListState::default().with_offset(app.tag_picker_scroll_offset);
    state.select(Some(app.tag_picker_cursor.min(rows.len() - 1)));
    frame.render_stateful_widget(list, area, &mut state);
}

pub fn confirm_overlay(frame: &mut Frame<'_>, title: &str, prompt: String) {
    let area = confirm_rect(&prompt, frame.area());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(prompt)
            .block(Block::default().title(title).borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        area,
    );
}

pub fn notification(frame: &mut Frame<'_>, app: &AppState) {
    let Some(notification) = &app.notification else {
        return;
    };
    let elapsed = notification.created_at.elapsed();
    if elapsed.as_secs_f32() >= 3.0 {
        return;
    }
    let base_fg = if notification.is_error {
        Color::Red
    } else {
        Color::White
    };
    let style = if elapsed.as_secs_f32() < 1.5 {
        Style::default().fg(base_fg).bg(Color::DarkGray)
    } else if elapsed.as_secs_f32() < 2.4 {
        Style::default().fg(Color::Gray).bg(Color::DarkGray)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let area = bottom_right(frame.area(), 24, 3);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(notification.message.clone())
            .style(style)
            .block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn bottom_right(area: Rect, width: u16, height: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(height.min(area.height)),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(width.min(area.width)),
        ])
        .split(vertical[1]);
    horizontal[1]
}

fn top_centered_rect(width: u16, height: u16, area: Rect, top_offset: u16) -> Rect {
    let y = area.y + top_offset.min(area.height);
    let available_height = area.height.saturating_sub(top_offset);
    let clamped_width = width.min(area.width);
    Rect {
        x: area.x + area.width.saturating_sub(clamped_width) / 2,
        y,
        width: clamped_width,
        height: height.min(available_height),
    }
}

fn cmdline_prompt_area(area: Rect) -> Rect {
    top_centered_rect(
        CMDLINE_WIDTH,
        CMDLINE_PROMPT_HEIGHT,
        area,
        CMDLINE_TOP_OFFSET,
    )
}

fn cmdline_list_area(area: Rect, height: u16) -> Rect {
    top_centered_rect(
        CMDLINE_WIDTH,
        height,
        area,
        CMDLINE_TOP_OFFSET.saturating_add(CMDLINE_PROMPT_HEIGHT),
    )
}

fn confirm_rect(message: &str, area: Rect) -> Rect {
    if area.width == 0 || area.height == 0 {
        return area;
    }

    let max_width = area.width.min(72);
    let min_width = area.width.min(32);
    let desired_width = message
        .chars()
        .count()
        .saturating_add(4)
        .min(u16::MAX as usize) as u16;
    let width = desired_width.clamp(min_width, max_width);
    let inner_width = width.saturating_sub(2).max(1) as usize;
    let body_lines = wrapped_line_count(message, inner_width);
    let height = body_lines.saturating_add(2).max(5).min(area.height);

    centered_rect(width, height, area)
}

fn wrapped_line_count(message: &str, width: usize) -> u16 {
    message
        .lines()
        .map(|line| wrapped_line_count_for_line(line, width.max(1)))
        .sum::<usize>()
        .max(1)
        .min(u16::MAX as usize) as u16
}

fn wrapped_line_count_for_line(line: &str, width: usize) -> usize {
    let mut words = line.split_whitespace().peekable();
    if words.peek().is_none() {
        return 1;
    }

    let mut lines = 1;
    let mut current_width = 0;
    for word in words {
        let word_width = word.chars().count();
        if current_width == 0 {
            lines += word_width.saturating_sub(1) / width;
            current_width = word_width % width;
            if current_width == 0 {
                current_width = width;
            }
        } else if current_width + 1 + word_width <= width {
            current_width += 1 + word_width;
        } else {
            lines += 1 + word_width.saturating_sub(1) / width;
            current_width = word_width % width;
            if current_width == 0 {
                current_width = width;
            }
        }
    }
    lines
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(height.min(area.height)),
            Constraint::Min(0),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(width.min(area.width)),
            Constraint::Min(0),
        ])
        .split(vertical[1]);
    horizontal[1]
}

#[cfg(test)]
mod tests {
    use super::visible_scan_log_lines;

    #[test]
    fn scan_logs_show_bottom_rows_after_multiline_events() {
        let logs = vec!["one\ntwo\nthree".to_string(), "four".to_string()];

        assert_eq!(visible_scan_log_lines(&logs, 2), ["three", "four"]);
    }
}
