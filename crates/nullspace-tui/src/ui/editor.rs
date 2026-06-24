use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use crate::app::{AppState, Mode, RelatedPickerFocus};
use crate::ui::widgets::{self, EquationListRow};

const LABELS: [&str; 7] = [
    "Name",
    "Description",
    "LaTeX",
    "References",
    "Tags",
    "Variables",
    "Related",
];

const PLACEHOLDERS: [&str; 7] = [
    "",
    "",
    "E = mc^2",
    "Paper title | https://example.com",
    "physics, relativity",
    "E = energy\nm = mass\nc = speed of light",
    "Mass energy equivalence, Euler identity",
];

const MIN_TEXT_BOX_LINES: u16 = 1;
const MAX_TEXT_BOX_LINES: u16 = 10;
const BLOCK_CHROME_ROWS: u16 = 2;
const MULTILINE_FIELDS: [usize; 4] = [1, 2, 3, 5];

pub fn draw(frame: &mut Frame<'_>, app: &mut AppState) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(outer[0]);

    let row_constraints = app
        .editor
        .as_ref()
        .map(|editor| editor_row_constraints(editor, panes[0].width))
        .unwrap_or_else(default_row_constraints);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(panes[0]);

    let current_latex = app.editor.as_ref().map(|editor| editor.field_text(2));
    let latex_invalid = current_latex
        .as_deref()
        .is_some_and(|latex| app.preview_error.is_some() && app.preview_latex == latex);
    let cursor_visible = app.cursor_visible();

    if let Some(editor) = &mut app.editor {
        for (index, area) in rows.iter().enumerate() {
            let style = if editor.focus == index {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let title = if index == 6 {
                "Related (up/down select, enter open, r edit)"
            } else {
                LABELS[index]
            };
            let block = Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(if index == 2 && latex_invalid {
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
                } else {
                    style
                });
            if index == 6 {
                render_related_field(
                    frame,
                    *area,
                    block,
                    &editor.field_text(index),
                    editor.related_cursor,
                );
                continue;
            }
            editor.fields[index].set_block(block);
            editor.fields[index].set_cursor_line_style(Style::default());
            editor.fields[index].set_cursor_style(if index == editor.focus && cursor_visible {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            });
            if !PLACEHOLDERS[index].is_empty() {
                editor.fields[index].set_placeholder_text(PLACEHOLDERS[index]);
                editor.fields[index].set_placeholder_style(Style::default().fg(Color::DarkGray));
            }
            frame.render_widget(&editor.fields[index], *area);
        }
    }
    widgets::preview_pane(frame, panes[1], app, "Live preview");
    if matches!(app.mode, Mode::RelatedPicker) {
        draw_related_picker(frame, app);
    }
    if let Mode::ConfirmRemoveRelated(id) = app.mode {
        draw_remove_related_confirm(frame, app, id);
    }
    widgets::status_bar(frame, outer[1], app);
}

fn default_row_constraints() -> Vec<Constraint> {
    vec![
        Constraint::Length(3),
        Constraint::Length(MIN_TEXT_BOX_LINES + BLOCK_CHROME_ROWS),
        Constraint::Length(MIN_TEXT_BOX_LINES + BLOCK_CHROME_ROWS),
        Constraint::Length(MIN_TEXT_BOX_LINES + BLOCK_CHROME_ROWS),
        Constraint::Length(3),
        Constraint::Length(MIN_TEXT_BOX_LINES + BLOCK_CHROME_ROWS),
        Constraint::Min(3),
    ]
}

fn editor_row_constraints(editor: &crate::app::EditorState, width: u16) -> Vec<Constraint> {
    (0..7)
        .map(|index| match index {
            0 | 4 => Constraint::Length(3),
            6 => Constraint::Min(3),
            _ if MULTILINE_FIELDS.contains(&index) => {
                Constraint::Length(text_box_height(editor, index, width))
            }
            _ => Constraint::Length(MIN_TEXT_BOX_LINES + BLOCK_CHROME_ROWS),
        })
        .collect()
}

fn text_box_height(editor: &crate::app::EditorState, index: usize, width: u16) -> u16 {
    let mut textarea = editor.fields[index].clone();
    textarea.set_block(Block::default().borders(Borders::ALL));
    textarea.set_min_rows(MIN_TEXT_BOX_LINES + BLOCK_CHROME_ROWS);
    textarea.set_max_rows(MAX_TEXT_BOX_LINES + BLOCK_CHROME_ROWS);
    textarea.measure(width).preferred_rows
}

fn draw_related_picker(frame: &mut Frame<'_>, app: &mut AppState) {
    let Some(editor) = &app.editor else {
        return;
    };
    let query = editor.related_picker.query.clone();
    let query_cursor = editor.related_picker.query_cursor;
    let picker_cursor = editor.related_picker.cursor;
    let list_scroll_offset = editor.related_picker.list_scroll_offset;
    let selected = editor.related_picker.selected.clone();
    let focus = editor.related_picker.focus;
    let items = app.filtered_related_picker_items();
    let rows = items
        .iter()
        .map(|item| {
            let checked = if selected.contains(&item.id) {
                "[x]"
            } else {
                "[ ]"
            };
            EquationListRow::new(checked, item)
        })
        .collect::<Vec<_>>();
    let item_count = rows.len();

    let modal_height = frame.area().height.saturating_sub(4).clamp(12, 34);
    let area = centered_rect(88, modal_height, frame.area());
    frame.render_widget(Clear, area);
    let content_height = modal_height.saturating_sub(3);
    let preview_height = ((content_height as usize * 45) / 100)
        .saturating_sub(2)
        .max(3)
        .min(u16::MAX as usize) as u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(preview_height),
            Constraint::Min(1),
        ])
        .split(area);

    widgets::search_box(
        frame,
        chunks[0],
        widgets::SearchBox {
            title: "Search",
            label: "Query: ",
            query: &query,
            cursor: query_cursor,
            hint: "tab list  enter apply  esc cancel",
            focused: focus == RelatedPickerFocus::Search,
        },
    );

    widgets::preview_pane(frame, chunks[1], app, "Equation");

    if let Some(editor) = &mut app.editor {
        editor.related_picker.list_visible_height = chunks[2].height.saturating_sub(2);
    }

    let (list, mut state) = widgets::equation_list(
        &rows,
        (item_count > 0).then_some(picker_cursor.min(item_count.saturating_sub(1))),
        list_scroll_offset,
        "Related equations  tab search  space toggles  enter applies",
        focus == RelatedPickerFocus::List,
    );
    frame.render_stateful_widget(list, chunks[2], &mut state);
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

fn draw_remove_related_confirm(
    frame: &mut Frame<'_>,
    app: &AppState,
    id: nullspace_core::EquationId,
) {
    let name = app
        .items
        .iter()
        .find(|item| item.id == id)
        .map(|item| item.name.as_str())
        .unwrap_or("selected equation");
    let area = centered_rect(54, 5, frame.area());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(format!("Remove \"{name}\" from related equations? (y/n)"))
            .block(Block::default().title("Confirm").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_related_field(
    frame: &mut Frame<'_>,
    area: Rect,
    block: Block<'_>,
    value: &str,
    cursor: usize,
) {
    let names = related_names(value);
    if names.is_empty() {
        frame.render_widget(
            Paragraph::new("No related equations\n\nPress r to choose from the library")
                .style(Style::default().fg(Color::DarkGray))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
    } else {
        let items = names
            .iter()
            .map(|name| ListItem::new(Line::from(name.clone())))
            .collect::<Vec<_>>();
        let mut state = ListState::default();
        state.select(Some(cursor.min(items.len().saturating_sub(1))));
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
            .highlight_symbol("> ");
        frame.render_stateful_widget(list, area, &mut state);
    }
}

fn related_names(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}
