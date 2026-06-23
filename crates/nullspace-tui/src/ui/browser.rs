use nullspace_core::render::to_unicode_approx;
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::app::{AppState, CacheStatus, Mode};
use crate::ui::widgets;

pub fn draw(frame: &mut Frame<'_>, app: &mut AppState) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(outer[0]);

    const CACHE_MARKER_GUTTER: &str = "  ";

    let items = app
        .items
        .iter()
        .enumerate()
        .flat_map(|(index, item)| {
            let marker = match app.cache_status_for(&item.latex, item.px_height) {
                CacheStatus::Cached => "•",
                CacheStatus::Loading => app.cache_spinner(),
                CacheStatus::Empty => " ",
            };
            let item = ListItem::new(vec![
                Line::from(vec![
                    Span::styled(marker, Style::default().fg(Color::Yellow)),
                    Span::raw(" "),
                    Span::styled(&item.name, Style::default().add_modifier(Modifier::BOLD)),
                ]),
                Line::from(vec![
                    Span::raw(CACHE_MARKER_GUTTER),
                    Span::raw(to_unicode_approx(&item.latex)),
                ]),
            ]);
            let spacer = (index + 1 < app.items.len()).then(|| ListItem::new(Line::from("")));
            std::iter::once(item).chain(spacer)
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default();
    if !items.is_empty() {
        state.select(Some(app.cursor * 2));
    }
    let list = List::new(items)
        .block(
            Block::default()
                .title(app.browser_title())
                .borders(Borders::ALL),
        )
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, panes[0], &mut state);
    let preview_title = format!("Preview ({}px  +/- zoom)", app.preview_px);
    widgets::preview_pane(frame, panes[1], app, &preview_title);

    if matches!(app.mode, Mode::Search | Mode::VariableLookup) {
        let title = if matches!(app.mode, Mode::Search) {
            "Search"
        } else {
            "Variable lookup"
        };
        let prompt = format!("{}  enter apply  esc clear", app.browser_title());
        let area = centered_rect(64, 3, frame.area());
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(prompt).block(Block::default().title(title).borders(Borders::ALL)),
            area,
        );
    }

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
