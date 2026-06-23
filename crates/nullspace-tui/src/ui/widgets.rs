use nullspace_core::render::to_unicode_approx;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::app::{AppState, Mode};

pub fn preview_pane(frame: &mut Frame<'_>, area: Rect, app: &AppState, title: &str) {
    let mut lines = Vec::new();
    if let Some(error) = &app.preview_error {
        lines.push(Line::styled(
            format!("Render error: {error}"),
            Style::default().fg(Color::Red),
        ));
    } else if let Some(image) = &app.preview {
        lines.push(Line::from(format!(
            "Rendered image: {} x {} px",
            image.width(),
            image.height()
        )));
    } else {
        lines.push(Line::from("Rendering..."));
    }
    if !app.preview_latex.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::styled(
            to_unicode_approx(&app.preview_latex),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::from(""));
        lines.push(Line::from(app.preview_latex.clone()));
    }
    let paragraph = Paragraph::new(lines)
        .block(Block::default().title(title).borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

pub fn status_bar(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let help = match app.mode {
        Mode::Browser => "j/k move  enter/e edit  n new  d delete  q quit",
        Mode::Editor => "tab field  esc back",
        Mode::RelatedPicker => "j/k move  space toggle  enter apply  esc cancel",
        Mode::ConfirmDelete(_) => "y confirm  n/esc cancel",
        Mode::ConfirmRemoveRelated(_) => "y remove relation  n/esc cancel",
    };
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

pub fn notification(frame: &mut Frame<'_>, app: &AppState) {
    let Some(notification) = &app.notification else {
        return;
    };
    let elapsed = notification.created_at.elapsed();
    if elapsed.as_secs_f32() >= 3.0 {
        return;
    }
    let style = if elapsed.as_secs_f32() < 1.5 {
        Style::default().fg(Color::White).bg(Color::DarkGray)
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
