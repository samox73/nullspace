use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::app::{AppState, BrowserFilter, BrowserFilterFocus, CacheStatus, Mode};
use crate::ui::widgets::{self, EquationListRow};

const SEARCH_BOX_ROWS: u16 = 4;

pub fn draw(frame: &mut Frame<'_>, app: &mut AppState) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());
    let (list_area, preview_area) = crate::ui::content_panes(outer[0], app.layout);

    let rows = app
        .items
        .iter()
        .map(|item| {
            let marker = match app.cache_status_for(&item.latex, item.px_height) {
                CacheStatus::Cached => "•",
                CacheStatus::Loading => app.cache_spinner(),
                CacheStatus::Empty => " ",
            };
            EquationListRow::new(marker, item)
        })
        .collect::<Vec<_>>();
    let (search_area, list_area) = search_and_list_areas(list_area, app.mode);

    app.list_visible_height = list_area.height.saturating_sub(2);
    let (list, mut state) = widgets::equation_list(
        &rows,
        (!rows.is_empty()).then_some(app.cursor),
        app.list_scroll_offset,
        app.browser_title(),
        matches!(app.mode, Mode::Search) && app.browser_filter_focus == BrowserFilterFocus::List,
    );
    frame.render_stateful_widget(list, list_area, &mut state);
    let preview_title = if app.preview_render_px == app.preview_px {
        format!("Preview ({}px  +/- zoom)", app.preview_render_px)
    } else {
        format!(
            "Preview ({}px render, {}px zoom  +/-)",
            app.preview_render_px, app.preview_px
        )
    };
    widgets::preview_pane(frame, preview_area, app, &preview_title);

    if let Some(search_area) = search_area {
        draw_filter_prompt(frame, search_area, app);
    }

    if let Mode::ConfirmDelete(id) = app.mode {
        let name = app
            .items
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.name.as_str())
            .unwrap_or("selected equation");
        let prompt = format!("Delete \"{name}\"? (y/d/enter to confirm, n/esc to cancel)");
        let area = confirm_rect(&prompt, frame.area());
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(prompt)
                .block(Block::default().title("Confirm").borders(Borders::ALL))
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    widgets::status_bar(frame, outer[1], app);
}

fn search_and_list_areas(area: Rect, mode: Mode) -> (Option<Rect>, Rect) {
    if !matches!(mode, Mode::Search) {
        return (None, area);
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(SEARCH_BOX_ROWS), Constraint::Min(1)])
        .split(area);
    (Some(chunks[0]), chunks[1])
}

fn draw_filter_prompt(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let (title, label, query) = match &app.browser_filter {
        BrowserFilter::Search(query) => ("Search", "Query: ", query.as_str()),
        BrowserFilter::None => return,
    };
    widgets::search_box(
        frame,
        area,
        widgets::SearchBox {
            title,
            label,
            query,
            cursor: app.browser_filter_cursor,
            hint: "tab list  enter apply  esc clear",
            focused: app.browser_filter_focus == BrowserFilterFocus::Search,
        },
    );
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
