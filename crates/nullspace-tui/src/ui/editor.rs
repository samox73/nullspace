use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use std::collections::HashMap;

use crate::app::{AppState, EditorField, Mode, RelatedPickerFocus, ResolverRow};
use crate::ui::widgets::{self, EquationListRow};

const MIN_TEXT_BOX_LINES: u16 = 1;
const MAX_TEXT_BOX_LINES: u16 = 10;
const BLOCK_CHROME_ROWS: u16 = 2;

fn field_title(field: EditorField) -> &'static str {
    match field {
        EditorField::Name => "Name",
        EditorField::Description => "Description",
        EditorField::Latex => "LaTeX",
        EditorField::Assumptions => "Assumptions",
        EditorField::References => "References (a add, enter edit, o open, d remove)",
        EditorField::Tags => "Tags",
        EditorField::Variables => "Variables (a add, enter edit, d remove, c link, u unlink)",
        EditorField::Related => "Related (up/down select, enter open, r edit)",
    }
}

fn field_placeholder(field: EditorField) -> &'static str {
    match field {
        EditorField::Latex => "E = mc^2",
        EditorField::Assumptions => "non-relativistic limit, T << T_F",
        EditorField::Tags => "physics, relativity",
        EditorField::Variables => "E = energy\nm = mass\nc = speed of light",
        EditorField::Related => "Mass energy equivalence, Euler identity",
        EditorField::Name | EditorField::Description | EditorField::References => "",
    }
}

pub fn draw(frame: &mut Frame<'_>, app: &mut AppState) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());
    let (form_area, preview_area) = crate::ui::content_panes(outer[0], app.layout);

    let row_constraints = app
        .editor
        .as_ref()
        .map(|editor| editor_row_constraints(editor, form_area.width))
        .unwrap_or_else(default_row_constraints);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(form_area);

    let current_latex = app
        .editor
        .as_ref()
        .map(|editor| editor.field_text(EditorField::Latex));
    let latex_invalid = current_latex
        .as_deref()
        .is_some_and(|latex| app.preview_error.is_some() && app.preview_latex == latex);
    let cursor_visible = app.cursor_visible();

    let quantity_names: HashMap<_, _> = app
        .quantities
        .iter()
        .map(|(quantity, _)| (quantity.id, crate::app::quantity_label(quantity)))
        .collect();

    if let Some(editor) = &mut app.editor {
        for (field, area) in EditorField::ALL.into_iter().zip(rows.iter()) {
            let style = if editor.focus == field {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let block = Block::default()
                .title(field_title(field))
                .borders(Borders::ALL)
                .border_style(if field == EditorField::Latex && latex_invalid {
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
                } else {
                    style
                });
            match field {
                EditorField::References => render_reference_field(
                    frame,
                    *area,
                    block,
                    &editor.references,
                    editor.reference_cursor,
                ),
                EditorField::Related => render_related_field(
                    frame,
                    *area,
                    block,
                    &editor.field_text(field),
                    editor.related_cursor,
                ),
                EditorField::Variables => render_variable_field(
                    frame,
                    *area,
                    block,
                    &editor.variables,
                    editor.variable_cursor,
                    &quantity_names,
                ),
                EditorField::Name
                | EditorField::Description
                | EditorField::Latex
                | EditorField::Assumptions
                | EditorField::Tags => {
                    let placeholder = field_placeholder(field);
                    let focused = editor.focus;
                    let textarea = editor.field_mut(field);
                    textarea.set_block(block);
                    textarea.set_cursor_line_style(Style::default());
                    textarea.set_cursor_style(if field == focused && cursor_visible {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default()
                    });
                    if !placeholder.is_empty() {
                        textarea.set_placeholder_text(placeholder);
                        textarea.set_placeholder_style(Style::default().fg(Color::DarkGray));
                    }
                    frame.render_widget(&*textarea, *area);
                }
            }
        }
    }
    if !matches!(app.mode, Mode::RelatedPicker) {
        widgets::preview_pane(frame, preview_area, app, "Live preview");
    }
    if matches!(app.mode, Mode::RelatedPicker) {
        draw_related_picker(frame, app);
    }
    if matches!(app.mode, Mode::ReferenceEditor) {
        draw_reference_editor(frame, app);
    }
    if matches!(app.mode, Mode::VariableEditor) {
        draw_variable_editor(frame, app);
    }
    if matches!(app.mode, Mode::QuantityResolver) {
        draw_quantity_resolver(frame, app);
    }
    if let Mode::ConfirmRemoveRelated(id) = app.mode {
        draw_remove_related_confirm(frame, app, id);
    }
    if let Mode::ConfirmRemoveReference(index) = app.mode {
        draw_remove_reference_confirm(frame, app, index);
    }
    if let Mode::ConfirmRemoveVariable(index) = app.mode {
        draw_remove_variable_confirm(frame, app, index);
    }
    widgets::status_bar(frame, outer[1], app);
}

fn default_row_constraints() -> Vec<Constraint> {
    EditorField::ALL
        .map(|field| match field {
            EditorField::Name | EditorField::Tags => Constraint::Length(3),
            EditorField::Description | EditorField::Latex | EditorField::Assumptions => {
                Constraint::Length(MIN_TEXT_BOX_LINES + BLOCK_CHROME_ROWS)
            }
            EditorField::References => Constraint::Length(MIN_TEXT_BOX_LINES + BLOCK_CHROME_ROWS),
            EditorField::Variables => Constraint::Length(MIN_TEXT_BOX_LINES + BLOCK_CHROME_ROWS),
            EditorField::Related => Constraint::Min(3),
        })
        .to_vec()
}

fn editor_row_constraints(editor: &crate::app::EditorState, width: u16) -> Vec<Constraint> {
    EditorField::ALL
        .map(|field| match field {
            EditorField::Name | EditorField::Tags => Constraint::Length(3),
            EditorField::Description | EditorField::Latex | EditorField::Assumptions => {
                Constraint::Length(text_box_height(editor, field, width))
            }
            EditorField::References => Constraint::Length(reference_box_height(editor)),
            EditorField::Variables => Constraint::Length(variable_box_height(editor)),
            EditorField::Related => Constraint::Min(3),
        })
        .to_vec()
}

fn reference_box_height(editor: &crate::app::EditorState) -> u16 {
    let content = (editor.references.len() as u16).saturating_mul(2).max(2);
    (content + BLOCK_CHROME_ROWS).min(MAX_TEXT_BOX_LINES + BLOCK_CHROME_ROWS)
}

fn variable_box_height(editor: &crate::app::EditorState) -> u16 {
    let content = (editor.variables.len() as u16).max(2);
    (content + BLOCK_CHROME_ROWS).min(MAX_TEXT_BOX_LINES + BLOCK_CHROME_ROWS)
}

fn text_box_height(editor: &crate::app::EditorState, field: EditorField, width: u16) -> u16 {
    debug_assert!(field.is_multiline());
    let mut textarea = editor.field(field).clone();
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
            details: Vec::new(),
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

fn render_reference_field(
    frame: &mut Frame<'_>,
    area: Rect,
    block: Block<'_>,
    references: &[nullspace_core::Reference],
    cursor: usize,
) {
    if references.is_empty() {
        frame.render_widget(
            Paragraph::new(
                "No references\n\nPress a to add one (title, authors, year, DOI/URL, pages)",
            )
            .style(Style::default().fg(Color::DarkGray))
            .block(block)
            .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let items = references
        .iter()
        .map(|reference| {
            let citation = nullspace_core::reference::format_citation(reference);
            let link = nullspace_core::reference::reference_link(reference).unwrap_or_default();
            let detail = match reference.pages.as_deref() {
                Some(pages) if !pages.trim().is_empty() && !link.is_empty() => {
                    format!("{link}  pages {pages}")
                }
                Some(pages) if !pages.trim().is_empty() => format!("pages {pages}"),
                _ => link,
            };
            ListItem::new(vec![
                Line::from(citation),
                Line::styled(detail, Style::default().fg(Color::DarkGray)),
            ])
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default();
    state.select(Some(cursor.min(items.len().saturating_sub(1))));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_variable_field(
    frame: &mut Frame<'_>,
    area: Rect,
    block: Block<'_>,
    variables: &[nullspace_core::Variable],
    cursor: usize,
    quantity_names: &HashMap<nullspace_core::QuantityId, String>,
) {
    if variables.is_empty() {
        frame.render_widget(
            Paragraph::new("No variables\n\nPress a to add one")
                .style(Style::default().fg(Color::DarkGray))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let items = variables
        .iter()
        .map(|variable| {
            let text = if variable.description.trim().is_empty() {
                variable.symbol.clone()
            } else {
                format!("{} = {}", variable.symbol, variable.description)
            };
            let mut spans = vec![ratatui::text::Span::raw(text)];
            if let Some(label) = variable
                .quantity_id
                .and_then(|id| quantity_names.get(&id).cloned())
            {
                spans.push(ratatui::text::Span::styled(
                    format!("  -> {label}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default();
    state.select(Some(cursor.min(items.len().saturating_sub(1))));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_reference_editor(frame: &mut Frame<'_>, app: &mut AppState) {
    let Some(editor) = &mut app.editor else {
        return;
    };
    let area = centered_rect(70, 22, frame.area());
    frame.render_widget(Clear, area);
    let outer = Block::default().title("Reference").borders(Borders::ALL);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(inner);

    for index in 0..6 {
        let focused = editor.reference_form.focus == index;
        let style = if focused {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let block = Block::default()
            .title(crate::app::REFERENCE_FIELD_LABELS[index])
            .borders(Borders::ALL)
            .border_style(style);
        editor.reference_form.fields[index].set_block(block);
        editor.reference_form.fields[index].set_cursor_style(if focused {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        });
        frame.render_widget(&editor.reference_form.fields[index], rows[index]);
    }

    let hint = match &editor.reference_form.error {
        Some(err) => Line::styled(err.clone(), Style::default().fg(Color::Red)),
        None => Line::styled(
            "tab next - shift-tab prev - enter save - esc cancel",
            Style::default().fg(Color::DarkGray),
        ),
    };
    frame.render_widget(Paragraph::new(hint), rows[6]);
}

fn draw_variable_editor(frame: &mut Frame<'_>, app: &mut AppState) {
    let Some(editor) = &mut app.editor else {
        return;
    };
    let area = centered_rect(56, 10, frame.area());
    frame.render_widget(Clear, area);
    let outer = Block::default().title("Variable").borders(Borders::ALL);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(inner);

    for index in 0..2 {
        let focused = editor.variable_form.focus == index;
        let style = if focused {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let block = Block::default()
            .title(crate::app::VARIABLE_FIELD_LABELS[index])
            .borders(Borders::ALL)
            .border_style(style);
        editor.variable_form.fields[index].set_block(block);
        editor.variable_form.fields[index].set_cursor_style(if focused {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        });
        frame.render_widget(&editor.variable_form.fields[index], rows[index]);
    }

    let hint = match &editor.variable_form.error {
        Some(err) => Line::styled(err.clone(), Style::default().fg(Color::Red)),
        None => Line::styled(
            "tab next - shift-tab prev - enter save - esc cancel",
            Style::default().fg(Color::DarkGray),
        ),
    };
    frame.render_widget(Paragraph::new(hint), rows[2]);
}

fn draw_quantity_resolver(frame: &mut Frame<'_>, app: &mut AppState) {
    let Some(resolver) = &app.quantity_resolver else {
        return;
    };
    let Some(editor) = &app.editor else {
        return;
    };
    let Some(variable_index) = resolver.queue.get(resolver.position).copied() else {
        return;
    };
    let Some(variable) = editor.variables.get(variable_index) else {
        return;
    };
    let area = centered_rect(70, 20, frame.area());
    frame.render_widget(Clear, area);
    let outer = Block::default()
        .title(format!(
            "Link variable '{}'  ({} of {})",
            variable.symbol,
            resolver.position + 1,
            resolver.queue.len()
        ))
        .borders(Borders::ALL);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(inner);
    widgets::search_box(
        frame,
        chunks[0],
        widgets::SearchBox {
            title: "Filter",
            label: "Filter: ",
            query: &resolver.query,
            cursor: resolver.query_cursor,
            hint: "enter choose  esc skip",
            details: Vec::new(),
            focused: true,
        },
    );
    let rows = app.resolver_rows();
    let items = rows
        .iter()
        .map(|row| match row {
            ResolverRow::CreateNew => ListItem::new(Line::styled(
                format!("Create new quantity '{}'", variable.symbol),
                Style::default().fg(Color::Yellow),
            )),
            ResolverRow::Existing(id) => {
                let quantity = app
                    .quantities
                    .iter()
                    .find(|(quantity, _)| quantity.id == *id);
                let label = quantity
                    .map(|(quantity, _)| crate::app::quantity_label(quantity))
                    .unwrap_or_else(|| "unknown quantity".to_string());
                let detail = quantity
                    .map(|(quantity, _)| {
                        [quantity.units.as_str(), quantity.description.as_str()]
                            .into_iter()
                            .filter(|part| !part.trim().is_empty())
                            .collect::<Vec<_>>()
                            .join(" - ")
                    })
                    .unwrap_or_default();
                ListItem::new(vec![
                    Line::styled(label, Style::default().add_modifier(Modifier::BOLD)),
                    Line::styled(detail, Style::default().fg(Color::DarkGray)),
                ])
            }
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default();
    if !items.is_empty() {
        state.select(Some(resolver.cursor.min(items.len() - 1)));
    }
    let list = List::new(items)
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, chunks[1], &mut state);
}

fn draw_remove_reference_confirm(frame: &mut Frame<'_>, app: &AppState, index: usize) {
    let citation = app
        .editor
        .as_ref()
        .and_then(|editor| editor.references.get(index))
        .map(nullspace_core::reference::format_citation)
        .unwrap_or_else(|| "this reference".to_string());
    let area = centered_rect(60, 5, frame.area());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(format!("Remove reference \"{citation}\"? (y/n)"))
            .block(Block::default().title("Confirm").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_remove_variable_confirm(frame: &mut Frame<'_>, app: &AppState, index: usize) {
    let symbol = app
        .editor
        .as_ref()
        .and_then(|editor| editor.variables.get(index))
        .map(|variable| variable.symbol.as_str())
        .unwrap_or("this variable");
    let area = centered_rect(54, 5, frame.area());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(format!("Remove variable \"{symbol}\"? (y/n)"))
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
