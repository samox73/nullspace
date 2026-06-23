use nullspace_core::render::to_unicode_approx;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect, Size},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};
use ratatui_image::{Resize, StatefulImage};

use crate::app::{AppState, Mode};

pub fn preview_pane(frame: &mut Frame<'_>, area: Rect, app: &mut AppState, title: &str) {
    let block = Block::default().title(title).borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(protocol) = &mut app.preview_protocol {
        let error_height = if app.preview_error.is_some() {
            4.min(inner.height)
        } else {
            0
        };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(error_height)])
            .split(inner);
        frame.render_widget(Clear, chunks[0]);
        let image_area = centered_image_area(protocol, chunks[0]);
        frame.render_stateful_widget(StatefulImage::default(), image_area, protocol);
        if let Some(error) = &app.preview_error {
            frame.render_widget(render_stale_warning(error), chunks[1]);
        }
        return;
    }

    if let Some(error) = &app.preview_error {
        frame.render_widget(render_error(error), inner);
        return;
    }

    let caption_height = if app.preview_latex.is_empty() { 0 } else { 5 }.min(inner.height);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(caption_height)])
        .split(inner);

    frame.render_widget(Paragraph::new("Rendering..."), chunks[0]);

    if !app.preview_latex.is_empty() {
        let mut lines = Vec::new();
        lines.push(Line::from(""));
        lines.push(Line::styled(
            to_unicode_approx(&app.preview_latex),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::from(""));
        lines.push(Line::from(app.preview_latex.clone()));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), chunks[1]);
    }
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
    let help = match app.mode {
        Mode::Browser => "j/k move  / search  v symbol  enter/e edit  n new  d delete  q quit",
        Mode::Search => "type search  enter apply  esc clear",
        Mode::VariableLookup => "type symbol  enter apply  esc clear",
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
