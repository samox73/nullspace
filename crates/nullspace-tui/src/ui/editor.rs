use ratatui::{
    layout::{Constraint, Direction, Layout, Margin, Rect},
    prelude::Position,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use crate::app::{AppState, Mode};
use crate::ui::widgets;

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

pub fn draw(frame: &mut Frame<'_>, app: &mut AppState) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(outer[0]);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Min(3),
        ])
        .split(panes[0]);

    if let Some(editor) = &app.editor {
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
                .border_style(style);
            if index == 6 {
                render_related_field(
                    frame,
                    *area,
                    block,
                    &editor.fields[index],
                    editor.related_cursor,
                );
                continue;
            }
            let content = if editor.fields[index].is_empty() && !PLACEHOLDERS[index].is_empty() {
                PLACEHOLDERS[index].to_string()
            } else {
                editor.fields[index].clone()
            };
            frame.render_widget(
                Paragraph::new(content)
                    .style(if editor.fields[index].is_empty() {
                        Style::default().fg(Color::DarkGray)
                    } else {
                        Style::default()
                    })
                    .block(block)
                    .wrap(Wrap { trim: false }),
                *area,
            );
        }
        if editor.focus != 6 {
            let focused = rows[editor.focus].inner(Margin {
                vertical: 1,
                horizontal: 1,
            });
            let (line, column) =
                cursor_line_column(&editor.fields[editor.focus], editor.cursors[editor.focus]);
            if focused.width > 0 && focused.height > 0 {
                frame.set_cursor_position(Position::new(
                    focused.x + (column as u16).min(focused.width.saturating_sub(1)),
                    focused.y + (line as u16).min(focused.height.saturating_sub(1)),
                ));
            }
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

fn draw_related_picker(frame: &mut Frame<'_>, app: &AppState) {
    let Some(editor) = &app.editor else {
        return;
    };
    let items = app.filtered_related_picker_items();
    let area = centered_rect(76, 20, frame.area());
    frame.render_widget(Clear, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);

    let search = if editor.related_picker.query.is_empty() {
        "Type to fuzzy search by name, description, or LaTeX".to_string()
    } else {
        format!("Search: {}", editor.related_picker.query)
    };
    frame.render_widget(
        Paragraph::new(search).block(Block::default().title("Search").borders(Borders::ALL)),
        chunks[0],
    );

    let list_items = items
        .iter()
        .map(|item| {
            let checked = if editor.related_picker.selected.contains(&item.id) {
                "[x]"
            } else {
                "[ ]"
            };
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(checked, Style::default().fg(Color::Yellow)),
                    Span::raw(" "),
                    Span::styled(&item.name, Style::default().add_modifier(Modifier::BOLD)),
                ]),
                Line::styled(&item.description, Style::default().fg(Color::DarkGray)),
                Line::styled(&item.latex, Style::default().fg(Color::DarkGray)),
            ])
        })
        .collect::<Vec<_>>();

    let mut state = ListState::default();
    if !list_items.is_empty() {
        state.select(Some(editor.related_picker.cursor.min(list_items.len() - 1)));
    }
    let list = List::new(list_items)
        .block(
            Block::default()
                .title("Related equations  space toggles, enter applies")
                .borders(Borders::ALL),
        )
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, chunks[1], &mut state);
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

fn cursor_line_column(value: &str, cursor: usize) -> (usize, usize) {
    let before = &value[..cursor.min(value.len())];
    let line = before.bytes().filter(|byte| *byte == b'\n').count();
    let column = before
        .rsplit('\n')
        .next()
        .map(|segment| segment.chars().count())
        .unwrap_or(0);
    (line, column)
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
