use super::text::{set_textarea_text, textarea_from_text, textarea_lines, textarea_text};
use super::{
    CmdlineState, EditorField, command_matches, default_equation_px, effective_render_px,
    fuzzy_matches_item, is_supported_reference_target,
};
use crate::action::Action;
use crate::event::map_key;
use crate::graphics::{Graphics, TerminalCellSize};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nullspace_core::{Equation, EquationId, EquationSummary, Quantity, Variable};
use ratatui::layout::Size;
use std::path::PathBuf;
use std::time::Duration;

#[test]
fn textarea_round_trips_multiline_text() {
    let text = "a\nb\nc";
    let textarea = textarea_from_text(text);
    assert_eq!(textarea_text(&textarea), text);
}

#[test]
fn textarea_lines_preserve_trailing_empty_line() {
    assert_eq!(textarea_lines("a\n"), ["a".to_string(), String::new()]);
}

#[test]
fn editor_field_all_matches_discriminants() {
    for (index, field) in EditorField::ALL.iter().enumerate() {
        assert_eq!(field.index(), index);
    }
}

#[test]
fn related_picker_search_matches_name_or_latex_only() {
    let description_only = EquationSummary {
        id: EquationId::new(),
        name: "BCS gap relation".to_string(),
        description: "Mentions Debye in prose".to_string(),
        latex: "\\Delta = 1.76 k_B T_c".to_string(),
        unicode_approx: "Δ = 1.76 k_B T_c".to_string(),
        px_height: 48,
    };
    let actual_match = EquationSummary {
        id: EquationId::new(),
        name: "Debye heat capacity".to_string(),
        description: "Low-temperature lattice heat capacity".to_string(),
        latex: "C_V = \\beta T^3".to_string(),
        unicode_approx: "C_V = β T³".to_string(),
        px_height: 48,
    };

    assert!(!fuzzy_matches_item("Debye", &description_only));
    assert!(fuzzy_matches_item("Debye", &actual_match));
}

#[test]
fn command_matching_filters_by_prefix() {
    assert_eq!(
        command_matches(""),
        [
            "delete",
            "equations",
            "exit",
            "new",
            "openReference",
            "quantities",
            "rescan",
            "scan",
            "search",
            "tags",
            "trash"
        ]
    );
    assert_eq!(command_matches("o"), ["openReference"]);
    assert_eq!(command_matches("q"), ["quantities"]);
    assert_eq!(command_matches("s"), ["scan", "search"]);
    assert_eq!(command_matches("t"), ["tags", "trash"]);
    assert_eq!(command_matches("D"), ["delete"]);
}

#[test]
fn reference_open_targets_allow_https_and_local_files() {
    assert!(is_supported_reference_target("https://example.test"));
    assert!(is_supported_reference_target("/tmp/paper.pdf"));
    assert!(is_supported_reference_target("paper.pdf"));
    assert!(is_supported_reference_target("file:///tmp/paper.pdf"));
    assert!(!is_supported_reference_target("http://example.test"));
    assert!(!is_supported_reference_target(
        "ftp://example.test/file.pdf"
    ));
}

#[test]
fn cmdline_accept_completes_active_match() {
    let mut app = test_app();
    app.mode = super::Mode::Cmdline;
    app.cmdline = Some(CmdlineState {
        input: "se".to_string(),
        cursor: 2,
        selected: 0,
        return_mode: super::Mode::Browser,
    });

    app.accept_cmdline();

    let cmdline = app.cmdline.expect("cmdline should remain open");
    assert_eq!(cmdline.input, "search");
    assert_eq!(cmdline.cursor, "search".len());
}

#[test]
fn cmdline_selection_cycles_through_matches() {
    let mut app = test_app_with_cmdline("e");

    app.input_cmdline(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Down,
        crossterm::event::KeyModifiers::NONE,
    ));
    app.execute_cmdline();

    assert!(app.should_quit);
}

#[test]
fn execute_cmdline_exit_quits() {
    let mut app = test_app_with_cmdline("exit");

    app.execute_cmdline();

    assert!(app.should_quit);
    assert!(app.cmdline.is_none());
}

#[test]
fn execute_cmdline_new_opens_editor() {
    let mut app = test_app_with_cmdline("new");

    app.execute_cmdline();

    assert!(matches!(app.mode, super::Mode::Editor));
    assert!(app.editor.is_some());
    assert!(app.cmdline.is_none());
}

#[test]
fn execute_cmdline_delete_opens_confirm_delete() {
    let mut app = test_app_with_cmdline("delete");

    app.execute_cmdline();

    assert!(matches!(app.mode, super::Mode::ConfirmDelete(_)));
    assert!(app.cmdline.is_none());
}

#[test]
fn execute_cmdline_search_enters_search() {
    let mut app = test_app_with_cmdline("search");

    app.execute_cmdline();

    assert!(matches!(app.mode, super::Mode::Search));
    assert!(app.cmdline.is_none());
}

#[test]
fn execute_cmdline_trash_opens_trash() {
    let mut app = test_app_with_cmdline("trash");

    app.execute_cmdline();

    assert!(matches!(app.mode, super::Mode::Trash));
    assert!(app.cmdline.is_none());
}

#[test]
fn execute_cmdline_tags_opens_tag_picker() {
    let mut app = test_app_with_cmdline("tags");

    app.execute_cmdline();

    assert!(matches!(app.mode, super::Mode::TagPicker));
    assert!(app.cmdline.is_none());
}

#[test]
fn cmdline_cancel_from_quantities_returns_to_quantities() {
    let mut app = test_app();
    app.apply(Action::OpenQuantities);
    app.apply(Action::OpenCmdline);

    app.apply(Action::CmdlineCancel);

    assert!(matches!(app.mode, super::Mode::QuantityPicker));
    assert!(app.cmdline.is_none());
}

#[test]
fn quantity_picker_escape_returns_to_previous_view() {
    let mut app = test_app();
    app.browser_filter = super::BrowserFilter::Search("energy".to_string());
    app.apply(Action::OpenQuantities);

    let action = map_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &app);
    app.apply(action);

    assert!(matches!(app.mode, super::Mode::Browser));
    assert_eq!(
        app.browser_filter,
        super::BrowserFilter::Search("energy".to_string())
    );
}

#[test]
fn quantity_picker_q_quits() {
    let mut app = test_app();
    app.apply(Action::OpenQuantities);

    let action = map_key(key('q'), &app);
    app.apply(action);

    assert!(app.should_quit);
}

#[test]
fn quantity_equation_list_escape_returns_to_quantity_picker() {
    let mut app = test_app();
    let quantity = Quantity::new("E".to_string());
    app.store.insert_quantity(&quantity).unwrap();
    app.reload().unwrap();
    app.apply(Action::OpenQuantities);

    app.apply(Action::QuantityPickerApply);
    assert!(matches!(app.mode, super::Mode::Browser));
    assert!(matches!(
        app.browser_filter,
        super::BrowserFilter::Quantity { id, .. } if id == quantity.id
    ));

    let action = map_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &app);
    app.apply(action);

    assert!(matches!(app.mode, super::Mode::QuantityPicker));
    assert_eq!(app.browser_filter, super::BrowserFilter::None);
}

#[test]
fn variable_enter_opens_linked_quantity() {
    let mut app = test_app();
    let quantity = Quantity::new("E".to_string());
    app.store.insert_quantity(&quantity).unwrap();
    app.reload().unwrap();
    app.open_editor(None);
    {
        let editor = app.editor.as_mut().unwrap();
        editor.focus = EditorField::Variables;
        editor.variables = vec![Variable {
            symbol: "E".to_string(),
            description: "energy".to_string(),
            quantity_id: Some(quantity.id),
        }];
    }

    app.input_editor(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(matches!(app.mode, super::Mode::QuantityPicker));
    assert_eq!(app.quantities[app.quantity_cursor].0.id, quantity.id);
    assert!(app.editor.is_some());
}

#[test]
fn inactive_editor_jk_moves_between_fields() {
    let mut app = test_app();
    app.open_editor(None);

    app.apply(map_key(key('j'), &app));
    assert_eq!(app.editor.as_ref().unwrap().focus, EditorField::Description);

    app.apply(map_key(key('k'), &app));
    assert_eq!(app.editor.as_ref().unwrap().focus, EditorField::Name);
    assert!(!app.editor.as_ref().unwrap().active);
}

#[test]
fn editor_enter_activates_and_escape_deactivates_field() {
    let mut app = test_app();
    app.open_editor(None);

    app.apply(map_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &app,
    ));
    assert!(app.editor.as_ref().unwrap().active);

    app.apply(map_key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &app,
    ));
    assert!(matches!(app.mode, super::Mode::Editor));
    assert!(!app.editor.as_ref().unwrap().active);
}

#[test]
fn active_editor_jk_goes_to_field_input() {
    let mut app = test_app();
    app.open_editor(None);
    app.apply(map_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &app,
    ));

    app.apply(map_key(key('j'), &app));

    let editor = app.editor.as_ref().unwrap();
    assert_eq!(editor.focus, EditorField::Name);
    assert_eq!(editor.field_text(EditorField::Name), "j");
}

#[test]
fn variable_quantity_picker_escape_returns_to_editor() {
    let mut app = test_app();
    let quantity = Quantity::new("E".to_string());
    app.store.insert_quantity(&quantity).unwrap();
    app.reload().unwrap();
    app.open_editor(None);
    {
        let editor = app.editor.as_mut().unwrap();
        editor.focus = EditorField::Variables;
        editor.variables = vec![Variable {
            symbol: "E".to_string(),
            description: "energy".to_string(),
            quantity_id: Some(quantity.id),
        }];
    }
    app.input_editor(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let action = map_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &app);
    app.apply(action);

    assert!(matches!(app.mode, super::Mode::Editor));
    assert!(app.editor.is_some());
}

#[test]
fn variable_quantity_drilldown_restores_original_equation_list() {
    let mut app = test_app();
    let quantity = Quantity::new("E".to_string());
    let mut equation = Equation::new("Energy test".to_string(), "E_{test} = x".to_string());
    equation.variables = vec![Variable {
        symbol: "E".to_string(),
        description: "energy".to_string(),
        quantity_id: Some(quantity.id),
    }];
    let equation_id = equation.id;
    app.store.insert_quantity(&quantity).unwrap();
    app.store.insert(&equation).unwrap();
    app.reload().unwrap();
    app.cursor = app
        .items
        .iter()
        .position(|item| item.id == equation_id)
        .unwrap();
    app.apply(Action::OpenCurrent);
    app.editor.as_mut().unwrap().focus = EditorField::Variables;

    app.input_editor(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.apply(Action::QuantityPickerApply);
    app.apply(map_key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &app,
    ));
    app.apply(map_key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &app,
    ));
    app.apply(map_key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &app,
    ));

    assert!(matches!(app.mode, super::Mode::Browser));
    assert_eq!(app.browser_filter, super::BrowserFilter::None);
    assert_eq!(app.selected_id(), Some(equation_id));
}

#[test]
fn deep_quantity_drilldown_unwinds_without_empty_editor() {
    let mut app = test_app();
    let quantity = Quantity::new("E".to_string());
    let mut equation = Equation::new("Energy test".to_string(), "E_{deep} = x".to_string());
    equation.variables = vec![Variable {
        symbol: "E".to_string(),
        description: "energy".to_string(),
        quantity_id: Some(quantity.id),
    }];
    let equation_id = equation.id;
    app.store.insert_quantity(&quantity).unwrap();
    app.store.insert(&equation).unwrap();
    app.reload().unwrap();
    app.cursor = app
        .items
        .iter()
        .position(|item| item.id == equation_id)
        .unwrap();
    app.apply(Action::OpenCurrent);
    app.editor.as_mut().unwrap().focus = EditorField::Variables;

    app.input_editor(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.apply(Action::QuantityPickerApply);
    app.apply(Action::OpenCurrent);
    assert!(matches!(app.mode, super::Mode::Editor));
    assert!(app.editor.is_some());

    app.apply(map_key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &app,
    ));
    assert!(matches!(app.mode, super::Mode::Browser));
    assert!(matches!(
        app.browser_filter,
        super::BrowserFilter::Quantity { id, .. } if id == quantity.id
    ));

    app.apply(map_key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &app,
    ));
    assert!(matches!(app.mode, super::Mode::QuantityPicker));

    app.apply(map_key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &app,
    ));
    assert!(matches!(app.mode, super::Mode::Editor));
    assert!(app.editor.is_some());

    app.apply(map_key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &app,
    ));
    assert!(matches!(app.mode, super::Mode::Browser));
    assert_eq!(app.browser_filter, super::BrowserFilter::None);
    assert_eq!(app.selected_id(), Some(equation_id));
    assert!(!app.preview_latex.is_empty());
}

#[test]
fn variable_enter_without_quantity_is_noop() {
    let mut app = test_app();
    app.open_editor(None);
    {
        let editor = app.editor.as_mut().unwrap();
        editor.focus = EditorField::Variables;
        editor.variables = vec![Variable {
            symbol: "x".to_string(),
            description: "position".to_string(),
            quantity_id: None,
        }];
    }

    app.input_editor(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(matches!(app.mode, super::Mode::Editor));
    assert!(app.editor.is_some());
}

#[test]
fn variable_e_opens_edit_view() {
    let mut app = test_app();
    app.open_editor(None);
    {
        let editor = app.editor.as_mut().unwrap();
        editor.focus = EditorField::Variables;
        editor.variables = vec![Variable {
            symbol: "x".to_string(),
            description: "position".to_string(),
            quantity_id: None,
        }];
    }

    app.input_editor(key('e'));

    assert!(matches!(app.mode, super::Mode::VariableEditor));
}

#[test]
fn variable_form_updates_list_display_and_saved_equation() {
    let mut app = test_app();
    app.open_editor(None);
    {
        let editor = app.editor.as_mut().unwrap();
        editor.set_field_text(EditorField::Name, "Velocity".to_string());
        editor.set_field_text(EditorField::Latex, "v = x/t".to_string());
        editor.focus = EditorField::Variables;
    }

    app.open_variable_form(None);
    {
        let editor = app.editor.as_mut().unwrap();
        set_textarea_text(
            editor.variable_form.fields.get_mut(0).unwrap(),
            "v".to_string(),
        );
        set_textarea_text(
            editor.variable_form.fields.get_mut(1).unwrap(),
            "velocity".to_string(),
        );
    }
    app.save_variable_form();

    let expected_variables = {
        let editor = app.editor.as_ref().unwrap();
        assert!(matches!(app.mode, super::Mode::Editor));
        assert_eq!(editor.variables.len(), 1);
        assert_eq!(editor.variables[0].symbol, "v");
        assert_eq!(editor.variables[0].description, "velocity");
        assert_eq!(editor.field_text(EditorField::Variables), "v = velocity");
        editor.variables.clone()
    };

    app.save_editor().unwrap();
    let saved_id = app.editor.as_ref().unwrap().editing.unwrap();
    let saved = app.store.get(saved_id).unwrap();
    assert_eq!(saved.variables, expected_variables);
}

#[test]
fn tag_picker_rows_sorted_alphabetically_with_untagged_first() {
    let mut app = test_app();
    app.untagged_count = 2;
    app.tag_counts = vec![
        ("polaron".to_string(), 3),
        ("DFT".to_string(), 5),
        ("diagmc".to_string(), 7),
    ];

    assert_eq!(
        app.tag_picker_rows(),
        vec![
            super::TagPickerRow::Untagged { count: 2 },
            super::TagPickerRow::Tag {
                name: "DFT".to_string(),
                count: 5,
            },
            super::TagPickerRow::Tag {
                name: "diagmc".to_string(),
                count: 7,
            },
            super::TagPickerRow::Tag {
                name: "polaron".to_string(),
                count: 3,
            },
        ]
    );
}

#[test]
fn tag_picker_rows_omits_untagged_when_zero() {
    let mut app = test_app();
    app.untagged_count = 0;
    app.tag_counts = vec![("dft".to_string(), 1)];

    assert_eq!(
        app.tag_picker_rows(),
        vec![super::TagPickerRow::Tag {
            name: "dft".to_string(),
            count: 1,
        }]
    );
}

#[test]
fn tag_picker_apply_sets_exact_tag_filter() {
    let mut app = test_app();
    insert_test_equation(&mut app, "Tagged exact", "tagged_exact = 1", &["dft"]);
    insert_test_equation(
        &mut app,
        "Tagged substring",
        "tagged_substring = 1",
        &["dft-plus-u"],
    );
    app.reload().unwrap();
    app.apply(Action::OpenTags);
    let rows = app.tag_picker_rows();
    app.tag_picker_cursor = rows
        .iter()
        .position(|row| {
            matches!(
                row,
                super::TagPickerRow::Tag { name, .. } if name == "dft"
            )
        })
        .unwrap();

    app.apply(Action::TagPickerApply);

    assert_eq!(
        app.browser_filter,
        super::BrowserFilter::Tag("dft".to_string())
    );
    assert!(app.items.iter().any(|item| item.name == "Tagged exact"));
    assert!(!app.items.iter().any(|item| item.name == "Tagged substring"));
}

#[test]
fn tag_picker_apply_untagged_row_sets_untagged_filter() {
    let mut app = test_app();
    insert_test_equation(&mut app, "App untagged", "app_untagged = 1", &[]);
    app.reload().unwrap();
    app.apply(Action::OpenTags);
    app.tag_picker_cursor = 0;

    app.apply(Action::TagPickerApply);

    assert_eq!(app.browser_filter, super::BrowserFilter::Untagged);
    assert!(app.items.iter().any(|item| item.name == "App untagged"));
    assert_eq!(app.items.len(), app.store.untagged().unwrap().len());
}

#[test]
fn tag_picker_cancel_leaves_filter_untouched() {
    let mut app = test_app();
    app.browser_filter = super::BrowserFilter::Search("energy".to_string());

    app.apply(Action::OpenTags);
    app.apply(Action::TagPickerCancel);

    assert_eq!(
        app.browser_filter,
        super::BrowserFilter::Search("energy".to_string())
    );
    assert!(matches!(app.mode, super::Mode::Browser));
}

#[test]
fn auto_link_classifies_variables() {
    let mut app = test_app();
    let e = Quantity::new("E".to_string());
    let mut g1 = Quantity::new("G".to_string());
    g1.name = "full Green's function".to_string();
    let mut g2 = Quantity::new("G".to_string());
    g2.name = "imaginary-time Green's function".to_string();
    app.store.insert_quantity(&e).unwrap();
    app.store.insert_quantity(&g1).unwrap();
    app.store.insert_quantity(&g2).unwrap();
    app.reload().unwrap();
    app.open_editor(None);
    {
        let editor = app.editor.as_mut().unwrap();
        editor.focus = EditorField::Variables;
        editor.variables = vec![
            Variable {
                symbol: "E".to_string(),
                description: "energy".to_string(),
                quantity_id: None,
            },
            Variable {
                symbol: "G".to_string(),
                description: "Green's function".to_string(),
                quantity_id: None,
            },
            Variable {
                symbol: "x".to_string(),
                description: "position".to_string(),
                quantity_id: None,
            },
        ];
    }

    app.link_variables_to_quantities().unwrap();

    let editor = app.editor.as_ref().unwrap();
    assert_eq!(editor.variables[0].quantity_id, Some(e.id));
    assert!(editor.variables[2].quantity_id.is_some());
    assert_eq!(
        app.store.quantities_by_symbol("x").unwrap()[0].description,
        "position"
    );
    assert!(matches!(app.mode, super::Mode::QuantityResolver));
    assert_eq!(app.quantity_resolver.as_ref().unwrap().queue, vec![1]);
}

#[test]
fn resolver_accept_links_candidate() {
    let mut app = test_app();
    let first = Quantity::new("G".to_string());
    let second = Quantity::new("G".to_string());
    app.store.insert_quantity(&first).unwrap();
    app.store.insert_quantity(&second).unwrap();
    app.reload().unwrap();
    app.open_editor(None);
    {
        let editor = app.editor.as_mut().unwrap();
        editor.focus = EditorField::Variables;
        editor.variables = vec![Variable {
            symbol: "G".to_string(),
            description: "Green's function".to_string(),
            quantity_id: None,
        }];
    }
    app.link_variables_to_quantities().unwrap();

    app.apply(Action::ResolverAccept);

    let editor = app.editor.as_ref().unwrap();
    assert!(
        [first.id, second.id]
            .into_iter()
            .any(|id| editor.variables[0].quantity_id == Some(id))
    );
    assert!(editor.dirty);
    assert!(matches!(app.mode, super::Mode::Editor));
}

#[test]
fn resolver_skip_is_counted_in_status() {
    let mut app = test_app();
    let energy = Quantity::new("E".to_string());
    app.store.insert_quantity(&energy).unwrap();
    app.store
        .insert_quantity(&Quantity::new("G".to_string()))
        .unwrap();
    app.store
        .insert_quantity(&Quantity::new("G".to_string()))
        .unwrap();
    app.reload().unwrap();
    app.open_editor(None);
    {
        let editor = app.editor.as_mut().unwrap();
        editor.focus = EditorField::Variables;
        editor.variables = vec![
            Variable {
                symbol: "E".to_string(),
                description: "energy".to_string(),
                quantity_id: None,
            },
            Variable {
                symbol: "G".to_string(),
                description: "Green's function".to_string(),
                quantity_id: None,
            },
        ];
    }
    app.link_variables_to_quantities().unwrap();

    app.apply(Action::ResolverSkip);

    assert!(matches!(app.mode, super::Mode::Editor));
    assert_eq!(
        app.status,
        "Variables linked: 1 linked, 0 created, 1 skipped"
    );
    let editor = app.editor.as_ref().unwrap();
    assert_eq!(editor.variables[0].quantity_id, Some(energy.id));
    assert_eq!(editor.variables[1].quantity_id, None);
}

#[test]
fn unlink_clears_quantity_id() {
    let mut app = test_app();
    let quantity = Quantity::new("E".to_string());
    app.open_editor(None);
    {
        let editor = app.editor.as_mut().unwrap();
        editor.focus = EditorField::Variables;
        editor.variables = vec![Variable {
            symbol: "E".to_string(),
            description: "energy".to_string(),
            quantity_id: Some(quantity.id),
        }];
    }

    app.input_editor(key('u'));

    let editor = app.editor.as_ref().unwrap();
    assert_eq!(editor.variables[0].quantity_id, None);
    assert!(editor.dirty);
}

#[test]
fn browser_title_reflects_tag_and_untagged_filters() {
    let mut app = test_app();

    app.browser_filter = super::BrowserFilter::Tag("dft".to_string());
    assert_eq!(app.browser_title(), "Tag: dft");

    app.browser_filter = super::BrowserFilter::Untagged;
    assert_eq!(app.browser_title(), "Untagged");
}

#[test]
fn execute_cmdline_unknown_returns_to_browser_with_status() {
    let mut app = test_app_with_cmdline("wat");

    app.execute_cmdline();

    assert!(matches!(app.mode, super::Mode::Browser));
    assert_eq!(app.status, "Unknown command: wat");
    assert!(app.cmdline.is_none());
}

#[test]
fn gg_prefix_maps_to_browser_top() {
    let mut app = test_app();
    app.cursor = app.items.len().saturating_sub(1);
    app.list_scroll_offset = app.cursor;

    let first_g = map_key(key('g'), &app);
    assert!(matches!(first_g, Action::StartGoPrefix));
    app.apply(first_g);
    assert!(app.vim_go_prefix);

    let second_g = map_key(key('g'), &app);
    assert!(matches!(second_g, Action::MoveToTop));
    app.apply(second_g);

    assert_eq!(app.cursor, 0);
    assert_eq!(app.list_scroll_offset, 0);
    assert!(!app.vim_go_prefix);
}

#[test]
fn shift_g_moves_browser_to_bottom() {
    let mut app = test_app();
    app.list_visible_height = 5;

    let action = map_key(key('G'), &app);
    app.apply(action);

    assert_eq!(app.cursor, app.items.len() - 1);
    assert_eq!(app.list_scroll_offset, app.items.len().saturating_sub(2));
}

#[test]
fn non_prefix_action_clears_gg_prefix() {
    let mut app = test_app();

    app.apply(Action::StartGoPrefix);
    app.apply(Action::None);

    assert!(!app.vim_go_prefix);
}

#[test]
fn question_mark_opens_and_closes_help() {
    let mut app = test_app();

    let open = map_key(key('?'), &app);
    assert!(matches!(open, Action::OpenHelp));
    app.apply(open);
    assert!(app.help_open);

    let close = map_key(key('?'), &app);
    assert!(matches!(close, Action::CloseHelp));
    app.apply(close);
    assert!(!app.help_open);
}

#[test]
fn help_modal_consumes_other_keys() {
    let mut app = test_app();
    app.help_open = true;

    assert!(matches!(map_key(key('j'), &app), Action::None));
    assert!(matches!(map_key(key('q'), &app), Action::None));
}

#[test]
fn effective_render_px_caps_to_preview_height() {
    let size = Some(Size {
        width: 80,
        height: 5,
    });
    let cell_size = TerminalCellSize {
        width: 10,
        height: 20,
    };

    assert_eq!(effective_render_px(512, size, cell_size), 100);
}

#[test]
fn effective_render_px_does_not_upscale_small_equations() {
    let size = Some(Size {
        width: 80,
        height: 20,
    });
    let cell_size = TerminalCellSize {
        width: 10,
        height: 20,
    };

    assert_eq!(effective_render_px(48, size, cell_size), 48);
}

#[test]
fn effective_render_px_uses_detected_cell_height() {
    let size = Some(Size {
        width: 80,
        height: 5,
    });
    let cell_size = TerminalCellSize {
        width: 9,
        height: 18,
    };

    assert_eq!(effective_render_px(512, size, cell_size), 90);
}

#[test]
fn effective_render_px_uses_full_detected_cell_box() {
    let size = Some(Size {
        width: 80,
        height: 5,
    });
    let cell_size = TerminalCellSize {
        width: 12,
        height: 26,
    };

    assert_eq!(effective_render_px(512, size, cell_size), 130);
}

#[test]
fn default_equation_px_is_five_cell_heights() {
    let cell_size = TerminalCellSize {
        width: 10,
        height: 20,
    };

    assert_eq!(default_equation_px(cell_size), 100);
}

#[test]
fn scan_review_suppresses_autosave() {
    let mut app = test_app();
    let before = app.store.all().unwrap().len();
    let equation = Equation::new("Scanned".to_string(), "E=mc^2".to_string());
    app.open_editor_with(Some(equation), None);
    app.scan_review = true;
    if let Some(editor) = &mut app.editor {
        editor.dirty = true;
        editor.last_change = std::time::Instant::now() - Duration::from_millis(400);
    }

    app.tick_render();

    assert_eq!(app.store.all().unwrap().len(), before);
}

#[test]
fn confirm_scan_imports_quantities_and_equation() {
    let mut app = test_app();
    let quantity = Quantity::new("E".to_string());
    let mut equation = Equation::new("Scanned energy".to_string(), "E_{scan}=m c^2".to_string());
    equation.variables = vec![Variable {
        symbol: "E".to_string(),
        description: "energy".to_string(),
        quantity_id: Some(quantity.id),
    }];
    app.start_scan(super::ScanAgent::Claude);
    app.scan.as_mut().unwrap().staged_quantities = vec![quantity.clone()];
    app.open_editor_with(Some(equation), None);
    if let Some(editor) = &mut app.editor {
        editor.last_saved_signature = String::new();
    }
    app.scan_review = true;

    app.confirm_scan().unwrap();

    assert!(!app.scan_review);
    assert!(matches!(app.mode, super::Mode::Browser));
    assert!(
        app.store
            .quantities()
            .unwrap()
            .iter()
            .any(|(stored, _)| stored.id == quantity.id)
    );
    assert!(
        app.store
            .all()
            .unwrap()
            .iter()
            .any(|stored| stored.name == "Scanned energy")
    );
}

#[test]
fn scan_settings_cycle_model_and_effort() {
    let mut app = test_app();
    app.start_scan(super::ScanAgent::Claude);

    assert_eq!(
        app.scan.as_ref().unwrap().settings_label(),
        "model: opus | intelligence: xhigh"
    );

    app.scan_cycle_model();
    assert_eq!(
        app.scan.as_ref().unwrap().settings_label(),
        "model: sonnet | intelligence: xhigh"
    );

    app.scan_cycle_effort();
    assert_eq!(
        app.scan.as_ref().unwrap().settings_label(),
        "model: sonnet | intelligence: max"
    );

    app.scan_cycle_model();
    assert_eq!(
        app.scan.as_ref().unwrap().settings_label(),
        "model: gpt-5.5 | intelligence: high"
    );
}

#[test]
fn scan_q_quits() {
    let mut app = test_app();
    app.start_scan(super::ScanAgent::Claude);

    assert!(matches!(map_key(key('q'), &app), Action::Quit));
}

#[test]
fn scan_review_zoom_persists_preview_size() {
    let mut app = test_app();
    let equation = Equation::new("Scanned zoom".to_string(), "E_{zoom}=m c^2".to_string());
    app.start_scan(super::ScanAgent::Claude);
    app.open_editor_with(Some(equation), None);
    if let Some(editor) = &mut app.editor {
        editor.last_saved_signature = String::new();
    }
    app.scan_review = true;
    let before = app.selected.as_ref().unwrap().px_height;

    app.adjust_zoom(true).unwrap();
    app.confirm_scan().unwrap();

    let saved = app
        .store
        .all()
        .unwrap()
        .into_iter()
        .find(|equation| equation.name == "Scanned zoom")
        .unwrap();
    assert_eq!(saved.px_height, before + 16);
}

fn test_app_with_cmdline(input: &str) -> super::AppState {
    let mut app = test_app();
    app.mode = super::Mode::Cmdline;
    app.cmdline = Some(CmdlineState {
        input: input.to_string(),
        cursor: input.len(),
        selected: 0,
        return_mode: super::Mode::Browser,
    });
    app
}

fn test_app() -> super::AppState {
    let path = test_db_path();
    let _ = std::fs::remove_file(&path);
    super::AppState::open(
        &path,
        Graphics::test(TerminalCellSize {
            width: 10,
            height: 20,
        }),
    )
    .expect("test app should open")
}

fn insert_test_equation(app: &mut super::AppState, name: &str, latex: &str, tags: &[&str]) {
    let mut eq = Equation::new(name.to_string(), latex.to_string());
    eq.tags = tags.iter().map(|tag| tag.to_string()).collect();
    app.store.insert(&eq).unwrap();
}

fn test_db_path() -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "nullspace-cmdline-test-{}-{:?}.sqlite3",
        std::process::id(),
        std::thread::current().id()
    ));
    path
}

fn key(ch: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)
}
