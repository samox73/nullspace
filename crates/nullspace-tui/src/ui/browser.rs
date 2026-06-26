use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    Frame,
};

use crate::app::{AppState, BrowserFilter, BrowserFilterFocus, CacheStatus, Mode};
use crate::ui::widgets::{self, EquationListRow};

const SEARCH_BOX_ROWS: u16 = 9;
const TAG_SUGGESTION_ROWS: usize = 5;

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
        widgets::confirm_overlay(frame, "Confirm", prompt);
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
            details: search_details(query, &app.tag_counts),
            focused: app.browser_filter_focus == BrowserFilterFocus::Search,
        },
    );
}

fn search_details(query: &str, tag_counts: &[(String, usize)]) -> Vec<String> {
    if query
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("tag:"))
    {
        let term = &query[4..];
        let term = term.trim().to_lowercase();
        let mut tags = tag_counts
            .iter()
            .filter(|(tag, _)| term.is_empty() || tag.to_lowercase().contains(&term))
            .take(TAG_SUGGESTION_ROWS)
            .map(|(tag, count)| format!("{count:>3} {tag}"))
            .collect::<Vec<_>>();
        if tags.is_empty() {
            tags.push("no matching tags".to_string());
        }
        return tags;
    }

    if query.trim().is_empty() {
        vec!["matchers: tag:  var:".to_string()]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::search_details;

    #[test]
    fn empty_search_shows_available_matchers() {
        assert_eq!(
            search_details("", &[]),
            vec!["matchers: tag:  var:".to_string()]
        );
    }

    #[test]
    fn tag_search_shows_tag_counts() {
        let tags = vec![
            ("diagmc".to_string(), 34),
            ("dft".to_string(), 14),
            ("polaron".to_string(), 5),
        ];

        assert_eq!(
            search_details("tag:", &tags),
            vec![
                " 34 diagmc".to_string(),
                " 14 dft".to_string(),
                "  5 polaron".to_string(),
            ]
        );
    }

    #[test]
    fn tag_search_filters_tag_counts() {
        let tags = vec![
            ("diagmc".to_string(), 34),
            ("dft".to_string(), 14),
            ("polaron".to_string(), 5),
        ];

        assert_eq!(search_details("TAG:df", &tags), vec![" 14 dft".to_string()]);
    }
}
