use nullspace_core::render::to_unicode_approx;
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::app::{AppState, Mode};
use crate::ui::widgets;

pub fn draw(frame: &mut Frame<'_>, app: &AppState) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(outer[0]);

    let items = app
        .items
        .iter()
        .map(|item| {
            ListItem::new(vec![
                Line::styled(&item.name, Style::default().add_modifier(Modifier::BOLD)),
                Line::from(vec![Span::styled(
                    &item.description,
                    Style::default().fg(Color::DarkGray),
                )]),
                Line::from(to_unicode_approx(&item.latex)),
            ])
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default();
    if !items.is_empty() {
        state.select(Some(app.cursor));
    }
    let list = List::new(items)
        .block(Block::default().title("Equations").borders(Borders::ALL))
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, panes[0], &mut state);
    widgets::preview_pane(frame, panes[1], app, "Preview");

    if let Mode::ConfirmDelete(id) = app.mode {
        let name = app
            .items
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.name.as_str())
            .unwrap_or("selected equation");
        let area = centered_rect(48, 5, frame.area());
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(format!("Delete \"{name}\"? (y/n)"))
                .block(Block::default().title("Confirm").borders(Borders::ALL)),
            area,
        );
    }

    widgets::status_bar(frame, outer[1], app);
}

fn centered_rect(width: u16, height: u16, area: ratatui::layout::Rect) -> ratatui::layout::Rect {
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
