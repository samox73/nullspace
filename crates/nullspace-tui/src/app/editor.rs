use super::*;

impl AppState {
    pub(super) fn open_editor(&mut self, id: Option<EquationId>) {
        let equation = id.and_then(|eq_id| self.store.get(eq_id).ok());
        self.open_editor_with(equation, id);
    }

    pub(super) fn open_editor_with(
        &mut self,
        equation: Option<Equation>,
        editing: Option<EquationId>,
    ) {
        self.selected = equation.clone();
        let initial_related = equation
            .as_ref()
            .map(|eq| eq.related.clone())
            .unwrap_or_default();
        let initial_references = equation
            .as_ref()
            .map(|eq| eq.references.clone())
            .unwrap_or_default();
        let initial_variables = equation
            .as_ref()
            .map(|eq| eq.variables.clone())
            .unwrap_or_default();
        let field_values = EditorField::ALL.map(|field| match (&equation, field) {
            (Some(eq), EditorField::Name) => eq.name.clone(),
            (Some(eq), EditorField::Description) => eq.description.clone(),
            (Some(eq), EditorField::Latex) => eq.latex.clone(),
            (Some(eq), EditorField::Assumptions) => eq.assumptions.clone(),
            (Some(_), EditorField::References) => String::new(),
            (Some(eq), EditorField::Tags) => eq.tags.join(", "),
            (Some(_), EditorField::Variables) => format_variables(&initial_variables),
            (Some(_), EditorField::Related) => format_related(&initial_related, &self.all_items),
            (None, _) => String::new(),
        });
        let fields = field_values
            .each_ref()
            .map(|value| textarea_from_text(value));
        self.editor = Some(EditorState {
            editing,
            last_saved_signature: fields_signature(
                &field_values,
                &initial_related,
                &initial_references,
                &initial_variables,
            ),
            fields,
            focus: EditorField::Name,
            active: false,
            related_cursor: 0,
            reference_cursor: 0,
            variable_cursor: 0,
            references: initial_references,
            variables: initial_variables,
            reference_form: ReferenceForm::empty(),
            variable_form: VariableForm::empty(),
            dirty: false,
            last_change: Instant::now(),
            related_picker: RelatedPickerState {
                cursor: 0,
                list_scroll_offset: 0,
                list_visible_height: 0,
                selected: Vec::new(),
                query: String::new(),
                query_cursor: 0,
                focus: RelatedPickerFocus::Search,
            },
            related: initial_related,
        });
        self.mode = Mode::Editor;
        self.schedule_selected();
    }

    pub(super) fn input_editor(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        let Some(editor) = &mut self.editor else {
            return;
        };
        let focused = editor.focus;
        match key.code {
            KeyCode::Char('r') if focused == EditorField::Related => {
                self.open_related_picker();
                return;
            }
            KeyCode::Char('a') if focused == EditorField::References => {
                self.open_reference_form(None);
                return;
            }
            KeyCode::Char('a') if focused == EditorField::Variables => {
                self.open_variable_form(None);
                return;
            }
            KeyCode::Char('k') if focused == EditorField::References => {
                editor.reference_cursor = editor.reference_cursor.saturating_sub(1);
                return;
            }
            KeyCode::Char('j') if focused == EditorField::References => {
                let max = editor.references.len().saturating_sub(1);
                editor.reference_cursor = (editor.reference_cursor + 1).min(max);
                return;
            }
            KeyCode::Char('d') if focused == EditorField::References => {
                if let Some(idx) = self.current_reference_index() {
                    self.mode = Mode::ConfirmRemoveReference(idx);
                }
                return;
            }
            KeyCode::Char('o') if focused == EditorField::References => {
                if let Err(err) = self.open_reference() {
                    self.report_error(err);
                }
                return;
            }
            KeyCode::Char('k') if focused == EditorField::Variables => {
                editor.variable_cursor = editor.variable_cursor.saturating_sub(1);
                return;
            }
            KeyCode::Char('j') if focused == EditorField::Variables => {
                let max = editor.variables.len().saturating_sub(1);
                editor.variable_cursor = (editor.variable_cursor + 1).min(max);
                return;
            }
            KeyCode::Char('d') if focused == EditorField::Variables => {
                if let Some(idx) = self.current_variable_index() {
                    self.mode = Mode::ConfirmRemoveVariable(idx);
                }
                return;
            }
            KeyCode::Char('e') if focused == EditorField::Variables => {
                if let Some(idx) = self.current_variable_index() {
                    self.open_variable_form(Some(idx));
                }
                return;
            }
            KeyCode::Char('c') if focused == EditorField::Variables => {
                if let Err(err) = self.link_variables_to_quantities() {
                    self.report_error(err);
                }
                return;
            }
            KeyCode::Char('u') if focused == EditorField::Variables => {
                if let Some(index) = self.current_variable_index()
                    && let Some(editor) = &mut self.editor
                {
                    editor.variables[index].quantity_id = None;
                    mark_editor_dirty(editor);
                }
                return;
            }
            KeyCode::Char('k') if focused == EditorField::Related => {
                editor.related_cursor = editor.related_cursor.saturating_sub(1);
                return;
            }
            KeyCode::Char('j') if focused == EditorField::Related => {
                let max = editor.related.len().saturating_sub(1);
                editor.related_cursor = (editor.related_cursor + 1).min(max);
                return;
            }
            KeyCode::Char('d') if focused == EditorField::Related => {
                if let Some(id) = self.current_related_id() {
                    self.mode = Mode::ConfirmRemoveRelated(id);
                }
                return;
            }
            KeyCode::Enter if matches!(focused, EditorField::Name | EditorField::Tags) => return,
            KeyCode::Enter if focused == EditorField::References => {
                if let Some(idx) = self.current_reference_index() {
                    self.open_reference_form(Some(idx));
                }
                return;
            }
            KeyCode::Enter if focused == EditorField::Variables => {
                if let Some(idx) = self.current_variable_index() {
                    match self.open_variable_quantity(idx) {
                        Ok(true) => {}
                        Ok(false) => {}
                        Err(err) => self.report_error(err),
                    }
                }
                return;
            }
            KeyCode::Enter if focused == EditorField::Related => {
                self.open_selected_related_detail();
                return;
            }
            _ if !focused.is_list() && editor.field_mut(focused).input(key) => {}
            _ => return,
        }
        mark_editor_dirty(editor);
        if editor.focus == EditorField::Latex {
            self.schedule_selected();
        }
    }

    pub fn filtered_related_picker_items(&self) -> Vec<&EquationSummary> {
        let Some(editor) = &self.editor else {
            return Vec::new();
        };
        related_picker_items_for(&self.all_items, editor.editing)
            .into_iter()
            .filter(|item| fuzzy_matches_item(&editor.related_picker.query, item))
            .collect()
    }

    pub(super) fn open_related_picker(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        if editor.focus != EditorField::Related {
            return;
        }
        editor.related_picker.selected = editor.related.clone();
        editor.related_picker.query.clear();
        editor.related_picker.query_cursor = 0;
        editor.related_picker.focus = RelatedPickerFocus::Search;
        let items = related_picker_items_for(&self.all_items, editor.editing);
        if editor.related_picker.cursor >= items.len() {
            editor.related_picker.cursor = items.len().saturating_sub(1);
        }
        editor.related_picker.list_scroll_offset = editor
            .related_picker
            .list_scroll_offset
            .min(editor.related_picker.cursor);
        self.mode = Mode::RelatedPicker;
        self.schedule_related_picker_preview();
    }

    pub(super) fn current_reference_index(&self) -> Option<usize> {
        let editor = self.editor.as_ref()?;
        if editor.focus != EditorField::References || editor.references.is_empty() {
            return None;
        }
        Some(editor.reference_cursor.min(editor.references.len() - 1))
    }

    pub(super) fn open_reference_form(&mut self, index: Option<usize>) {
        let form = {
            let Some(editor) = self.editor.as_ref() else {
                return;
            };
            match index {
                Some(i) if i < editor.references.len() => {
                    ReferenceForm::from_reference(&editor.references[i], i)
                }
                _ => ReferenceForm::empty(),
            }
        };
        if let Some(editor) = self.editor.as_mut() {
            editor.reference_form = form;
        }
        self.mode = Mode::ReferenceEditor;
    }

    pub(super) fn input_reference_form(&mut self, key: crossterm::event::KeyEvent) {
        if let Some(editor) = &mut self.editor {
            let focus = editor.reference_form.focus;
            editor.reference_form.fields[focus].input(key);
            editor.reference_form.error = None;
        }
    }

    pub(super) fn reference_form_next_field(&mut self) {
        if let Some(editor) = &mut self.editor {
            let n = editor.reference_form.fields.len();
            editor.reference_form.focus = (editor.reference_form.focus + 1) % n;
        }
    }

    pub(super) fn reference_form_prev_field(&mut self) {
        if let Some(editor) = &mut self.editor {
            let n = editor.reference_form.fields.len();
            editor.reference_form.focus = (editor.reference_form.focus + n - 1) % n;
        }
    }

    pub(super) fn save_reference_form(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let authors = editor.reference_form.text_at(0).trim().to_string();
        let year_raw = editor.reference_form.text_at(1).trim().to_string();
        let title = editor.reference_form.text_at(2).trim().to_string();
        let doi_raw = editor.reference_form.text_at(3).trim().to_string();
        let url_raw = editor.reference_form.text_at(4).trim().to_string();
        let pages_raw = editor.reference_form.text_at(5).trim().to_string();

        if title.is_empty() && authors.is_empty() {
            editor.reference_form.error = Some("Enter at least a title or authors".to_string());
            return;
        }
        let year = if year_raw.is_empty() {
            None
        } else {
            match year_raw.parse::<i32>() {
                Ok(y) => Some(y),
                Err(_) => {
                    editor.reference_form.error = Some("Year must be a number".to_string());
                    return;
                }
            }
        };

        let (doi, url) = if !doi_raw.is_empty() {
            let doi = normalize_doi(&doi_raw).unwrap_or(doi_raw);
            let url = (!url_raw.is_empty()).then_some(url_raw);
            (Some(doi), url)
        } else if let Some(doi) = normalize_doi(&url_raw) {
            (Some(doi), None)
        } else {
            (None, (!url_raw.is_empty()).then_some(url_raw))
        };
        let pages = if pages_raw.is_empty() {
            None
        } else {
            match normalize_pages(&pages_raw) {
                Some(pages) => Some(pages),
                None => {
                    editor.reference_form.error =
                        Some("Pages must look like 1, 5-7, or 2, 5-7".to_string());
                    return;
                }
            }
        };

        let reference = Reference {
            authors,
            year,
            title,
            doi,
            url,
            pages,
        };
        let editing = editor.reference_form.editing;
        match editing {
            Some(i) if i < editor.references.len() => editor.references[i] = reference,
            _ => editor.references.push(reference),
        }
        editor.reference_cursor = match editing {
            Some(i) => i.min(editor.references.len().saturating_sub(1)),
            None => editor.references.len().saturating_sub(1),
        };
        mark_editor_dirty(editor);
        self.mode = Mode::Editor;
    }

    pub(super) fn remove_reference(&mut self, index: usize) {
        if let Some(editor) = &mut self.editor
            && index < editor.references.len()
        {
            editor.references.remove(index);
            editor.reference_cursor = editor
                .reference_cursor
                .min(editor.references.len().saturating_sub(1));
            mark_editor_dirty(editor);
        }
    }

    pub(super) fn current_variable_index(&self) -> Option<usize> {
        let editor = self.editor.as_ref()?;
        if editor.focus != EditorField::Variables || editor.variables.is_empty() {
            return None;
        }
        Some(editor.variable_cursor.min(editor.variables.len() - 1))
    }

    pub(super) fn open_variable_form(&mut self, index: Option<usize>) {
        let form = {
            let Some(editor) = self.editor.as_ref() else {
                return;
            };
            match index {
                Some(i) if i < editor.variables.len() => {
                    VariableForm::from_variable(&editor.variables[i], i)
                }
                _ => VariableForm::empty(),
            }
        };
        if let Some(editor) = self.editor.as_mut() {
            editor.variable_form = form;
        }
        self.mode = Mode::VariableEditor;
    }

    pub(super) fn open_variable_quantity(&mut self, index: usize) -> anyhow::Result<bool> {
        let Some(id) = self
            .editor
            .as_ref()
            .and_then(|editor| editor.variables.get(index))
            .and_then(|variable| variable.quantity_id)
        else {
            return Ok(false);
        };
        let Some(label) = self
            .quantities
            .iter()
            .find(|(quantity, _)| quantity.id == id)
            .map(|(quantity, _)| quantity_label(quantity))
        else {
            return Ok(false);
        };
        if self.editor.as_ref().is_some_and(|editor| editor.dirty) {
            self.persist_editor(false)?;
        }
        self.quantities = self.store.quantities()?;
        if let Some(index) = self
            .quantities
            .iter()
            .position(|(quantity, _)| quantity.id == id)
        {
            self.move_quantity_cursor_to(index);
        }
        self.push_nav();
        self.mode = Mode::QuantityPicker;
        self.status = format!("Quantity: {label}");
        Ok(true)
    }

    pub(super) fn open_quantity_equations(
        &mut self,
        id: QuantityId,
        label: String,
    ) -> anyhow::Result<()> {
        self.browser_filter = BrowserFilter::Quantity { id, label };
        self.cursor = 0;
        self.refresh_items()?;
        self.status = format!("{}: {} match(es)", self.browser_title(), self.items.len());
        self.schedule_selected();
        Ok(())
    }

    pub(super) fn input_variable_form(&mut self, key: crossterm::event::KeyEvent) {
        if let Some(editor) = &mut self.editor {
            let focus = editor.variable_form.focus;
            editor.variable_form.fields[focus].input(key);
            editor.variable_form.error = None;
        }
    }

    pub(super) fn variable_form_next_field(&mut self) {
        if let Some(editor) = &mut self.editor {
            let n = editor.variable_form.fields.len();
            editor.variable_form.focus = (editor.variable_form.focus + 1) % n;
        }
    }

    pub(super) fn variable_form_prev_field(&mut self) {
        if let Some(editor) = &mut self.editor {
            let n = editor.variable_form.fields.len();
            editor.variable_form.focus = (editor.variable_form.focus + n - 1) % n;
        }
    }

    pub(super) fn input_quantity_form(&mut self, key: crossterm::event::KeyEvent) {
        if let Some(form) = &mut self.quantity_form {
            let focus = form.focus;
            form.fields[focus].input(key);
            form.error = None;
        }
    }

    pub(super) fn quantity_form_next_field(&mut self) {
        if let Some(form) = &mut self.quantity_form {
            form.focus = (form.focus + 1) % form.fields.len();
        }
    }

    pub(super) fn quantity_form_prev_field(&mut self) {
        if let Some(form) = &mut self.quantity_form {
            form.focus = form.focus.checked_sub(1).unwrap_or(form.fields.len() - 1);
        }
    }

    pub(super) fn save_quantity_form(&mut self) -> anyhow::Result<()> {
        let Some(form) = &mut self.quantity_form else {
            return Ok(());
        };
        let symbol = form.text_at(0).trim().to_string();
        if symbol.is_empty() {
            form.error = Some("Enter a symbol".to_string());
            return Ok(());
        }
        let mut quantity = form
            .editing
            .and_then(|id| {
                self.quantities
                    .iter()
                    .find(|(quantity, _)| quantity.id == id)
                    .map(|(quantity, _)| quantity.clone())
            })
            .unwrap_or_else(|| Quantity::new(symbol.clone()));
        quantity.symbol = symbol;
        quantity.name = form.text_at(1).trim().to_string();
        quantity.description = form.text_at(2).trim().to_string();
        quantity.units = form.text_at(3).trim().to_string();

        if form.editing.is_some() {
            self.store.update_quantity(&quantity)?;
        } else {
            self.store.insert_quantity(&quantity)?;
        }
        self.quantities = self.store.quantities()?;
        if let Some(index) = self
            .quantities
            .iter()
            .position(|(candidate, _)| candidate.id == quantity.id)
        {
            self.move_quantity_cursor_to(index);
        }
        self.quantity_form = None;
        self.mode = Mode::QuantityPicker;
        self.status = "Quantity saved".to_string();
        Ok(())
    }

    pub(super) fn save_variable_form(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let symbol = editor.variable_form.text_at(0).trim().to_string();
        let description = editor.variable_form.text_at(1).trim().to_string();
        if symbol.is_empty() {
            editor.variable_form.error = Some("Enter a symbol".to_string());
            return;
        }
        if editor
            .variables
            .iter()
            .enumerate()
            .any(|(index, variable)| {
                Some(index) != editor.variable_form.editing && variable.symbol == symbol
            })
        {
            editor.variable_form.error = Some("Symbol already exists".to_string());
            return;
        }

        let variable = Variable {
            symbol,
            description,
            quantity_id: editor
                .variable_form
                .editing
                .and_then(|i| editor.variables.get(i))
                .and_then(|variable| variable.quantity_id),
        };
        let editing = editor.variable_form.editing;
        match editing {
            Some(i) if i < editor.variables.len() => editor.variables[i] = variable,
            _ => editor.variables.push(variable),
        }
        editor.variable_cursor = match editing {
            Some(i) => i.min(editor.variables.len().saturating_sub(1)),
            None => editor.variables.len().saturating_sub(1),
        };
        let display = format_variables(&editor.variables);
        editor.set_field_text(EditorField::Variables, display);
        mark_editor_dirty(editor);
        self.mode = Mode::Editor;
    }

    pub fn link_variables_to_quantities(&mut self) -> anyhow::Result<()> {
        let Some(editor) = self.editor.as_ref() else {
            return Ok(());
        };
        if editor.focus != EditorField::Variables {
            return Ok(());
        }
        let known: HashSet<_> = self
            .quantities
            .iter()
            .map(|(quantity, _)| quantity.id)
            .collect();
        let variables = editor
            .variables
            .iter()
            .enumerate()
            .filter(|(_, variable)| !variable.quantity_id.is_some_and(|id| known.contains(&id)))
            .map(|(index, variable)| {
                (
                    index,
                    variable.symbol.trim().to_string(),
                    variable.description.clone(),
                )
            })
            .filter(|(_, symbol, _)| !symbol.is_empty())
            .collect::<Vec<_>>();

        let mut links = Vec::new();
        let mut queue = Vec::new();
        let mut linked = 0;
        let mut created = 0;
        for (index, symbol, description) in variables {
            let matches = self.store.quantities_by_symbol(&symbol)?;
            match matches.as_slice() {
                [] => {
                    let mut quantity = Quantity::new(symbol);
                    quantity.description = description;
                    self.store.insert_quantity(&quantity)?;
                    links.push((index, quantity.id));
                    created += 1;
                }
                [quantity] => {
                    links.push((index, quantity.id));
                    linked += 1;
                }
                _ => queue.push(index),
            }
        }
        if created > 0 {
            self.quantities = self.store.quantities()?;
        }
        if let Some(editor) = &mut self.editor {
            for (index, id) in links {
                if let Some(variable) = editor.variables.get_mut(index) {
                    variable.quantity_id = Some(id);
                }
            }
            if linked > 0 || created > 0 {
                mark_editor_dirty(editor);
            }
        }
        if queue.is_empty() {
            self.status = format!("Variables linked: {linked} linked, {created} created");
            return Ok(());
        }
        let cursor = self.initial_resolver_cursor(queue[0]);
        self.quantity_resolver = Some(QuantityResolverState {
            queue,
            position: 0,
            query: String::new(),
            query_cursor: 0,
            cursor,
            linked,
            created,
            skipped: 0,
        });
        self.mode = Mode::QuantityResolver;
        Ok(())
    }

    pub fn resolver_rows(&self) -> Vec<ResolverRow> {
        let Some(resolver) = &self.quantity_resolver else {
            return Vec::new();
        };
        let Some(editor) = &self.editor else {
            return Vec::new();
        };
        let Some(index) = resolver.queue.get(resolver.position).copied() else {
            return Vec::new();
        };
        let Some(variable) = editor.variables.get(index) else {
            return Vec::new();
        };
        let symbol = variable.symbol.trim();
        let mut rows = vec![ResolverRow::CreateNew];
        let exact = self
            .quantities
            .iter()
            .filter(|(quantity, _)| quantity.symbol == symbol)
            .map(|(quantity, _)| quantity.id)
            .collect::<Vec<_>>();
        rows.extend(exact.iter().copied().map(ResolverRow::Existing));
        let exact: HashSet<_> = exact.into_iter().collect();
        let query = resolver.query.trim().to_lowercase();
        rows.extend(
            self.quantities
                .iter()
                .filter(|(quantity, _)| !exact.contains(&quantity.id))
                .filter(|(quantity, _)| {
                    query.is_empty()
                        || format!(
                            "{} {} {}",
                            quantity.symbol, quantity.name, quantity.description
                        )
                        .to_lowercase()
                        .contains(&query)
                })
                .map(|(quantity, _)| ResolverRow::Existing(quantity.id)),
        );
        rows
    }

    pub(super) fn initial_resolver_cursor(&self, variable_index: usize) -> usize {
        let Some(editor) = &self.editor else {
            return 0;
        };
        let Some(variable) = editor.variables.get(variable_index) else {
            return 0;
        };
        if self
            .quantities
            .iter()
            .any(|(quantity, _)| quantity.symbol == variable.symbol.trim())
        {
            1
        } else {
            0
        }
    }

    pub(super) fn move_resolver_cursor(&mut self, down: bool) {
        let max = self.resolver_rows().len().saturating_sub(1);
        if let Some(resolver) = &mut self.quantity_resolver {
            resolver.cursor = if down {
                (resolver.cursor + 1).min(max)
            } else {
                resolver.cursor.saturating_sub(1)
            };
        }
    }

    pub(super) fn input_resolver(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        let Some(resolver) = &mut self.quantity_resolver else {
            return;
        };
        let mut changed = false;
        match key.code {
            KeyCode::Char(ch) => {
                resolver.query.insert(resolver.query_cursor, ch);
                resolver.query_cursor += ch.len_utf8();
                changed = true;
            }
            KeyCode::Backspace if resolver.query_cursor > 0 => {
                let previous = prev_boundary(&resolver.query, resolver.query_cursor);
                resolver.query.drain(previous..resolver.query_cursor);
                resolver.query_cursor = previous;
                changed = true;
            }
            KeyCode::Delete if resolver.query_cursor < resolver.query.len() => {
                let next = next_boundary(&resolver.query, resolver.query_cursor);
                resolver.query.drain(resolver.query_cursor..next);
                changed = true;
            }
            KeyCode::Left => {
                resolver.query_cursor = prev_boundary(&resolver.query, resolver.query_cursor)
            }
            KeyCode::Right => {
                resolver.query_cursor = next_boundary(&resolver.query, resolver.query_cursor)
            }
            KeyCode::Home => resolver.query_cursor = 0,
            KeyCode::End => resolver.query_cursor = resolver.query.len(),
            _ => {}
        }
        if changed {
            let index = self
                .quantity_resolver
                .as_ref()
                .and_then(|resolver| resolver.queue.get(resolver.position).copied());
            let cursor = index
                .map(|index| self.initial_resolver_cursor(index))
                .unwrap_or(0);
            let max = self.resolver_rows().len().saturating_sub(1);
            if let Some(resolver) = &mut self.quantity_resolver {
                resolver.cursor = cursor.min(max);
            }
        }
    }

    pub(super) fn accept_resolver(&mut self) -> anyhow::Result<()> {
        let row = self
            .resolver_rows()
            .get(
                self.quantity_resolver
                    .as_ref()
                    .map(|resolver| resolver.cursor)
                    .unwrap_or(0),
            )
            .cloned()
            .unwrap_or(ResolverRow::CreateNew);
        let Some((variable_index, symbol, description)) = self.current_resolver_variable() else {
            return Ok(());
        };
        let quantity_id = match row {
            ResolverRow::CreateNew => {
                let mut quantity = Quantity::new(symbol);
                quantity.description = description;
                let id = quantity.id;
                self.store.insert_quantity(&quantity)?;
                self.quantities = self.store.quantities()?;
                if let Some(resolver) = &mut self.quantity_resolver {
                    resolver.created += 1;
                }
                id
            }
            ResolverRow::Existing(id) => {
                if let Some(resolver) = &mut self.quantity_resolver {
                    resolver.linked += 1;
                }
                id
            }
        };
        if let Some(editor) = &mut self.editor
            && let Some(variable) = editor.variables.get_mut(variable_index)
        {
            variable.quantity_id = Some(quantity_id);
            mark_editor_dirty(editor);
        }
        self.advance_resolver();
        Ok(())
    }

    pub(super) fn current_resolver_variable(&self) -> Option<(usize, String, String)> {
        let resolver = self.quantity_resolver.as_ref()?;
        let editor = self.editor.as_ref()?;
        let index = resolver.queue.get(resolver.position).copied()?;
        let variable = editor.variables.get(index)?;
        Some((
            index,
            variable.symbol.trim().to_string(),
            variable.description.clone(),
        ))
    }

    pub(super) fn advance_resolver(&mut self) {
        let Some(mut resolver) = self.quantity_resolver.take() else {
            return;
        };
        resolver.position += 1;
        if resolver.position >= resolver.queue.len() {
            self.mode = Mode::Editor;
            self.status = format!(
                "Variables linked: {} linked, {} created, {} skipped",
                resolver.linked, resolver.created, resolver.skipped
            );
            return;
        }
        resolver.query.clear();
        resolver.query_cursor = 0;
        resolver.cursor = self.initial_resolver_cursor(resolver.queue[resolver.position]);
        self.quantity_resolver = Some(resolver);
    }

    pub(super) fn remove_variable(&mut self, index: usize) {
        if let Some(editor) = &mut self.editor
            && index < editor.variables.len()
        {
            editor.variables.remove(index);
            editor.variable_cursor = editor
                .variable_cursor
                .min(editor.variables.len().saturating_sub(1));
            let display = format_variables(&editor.variables);
            editor.set_field_text(EditorField::Variables, display);
            mark_editor_dirty(editor);
        }
    }

    pub(super) fn schedule_related_picker_preview(&mut self) {
        let Some((latex, px)) = self
            .related_picker_preview_item()
            .map(|item| (item.latex.clone(), RELATED_PICKER_PREVIEW_PX))
        else {
            return;
        };
        self.schedule_latex(latex, px);
    }

    pub(super) fn related_picker_preview_item(&self) -> Option<&EquationSummary> {
        let editor = self.editor.as_ref()?;
        related_picker_items_for(&self.all_items, editor.editing)
            .into_iter()
            .filter(|item| fuzzy_matches_item(&editor.related_picker.query, item))
            .nth(editor.related_picker.cursor)
    }

    pub(super) fn toggle_related_picker_focus(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.related_picker.focus = match editor.related_picker.focus {
            RelatedPickerFocus::Search => RelatedPickerFocus::List,
            RelatedPickerFocus::List => RelatedPickerFocus::Search,
        };
    }

    pub(super) fn move_related_picker_cursor(&mut self, down: bool) {
        if !self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.related_picker.focus == RelatedPickerFocus::List)
        {
            return;
        }
        let max = self.filtered_related_picker_items().len().saturating_sub(1);
        if let Some(editor) = &mut self.editor {
            if down {
                editor.related_picker.cursor = (editor.related_picker.cursor + 1).min(max);
                let visible =
                    list_visible_item_count(editor.related_picker.list_visible_height).max(1);
                if editor.related_picker.cursor
                    >= editor.related_picker.list_scroll_offset + visible
                {
                    editor.related_picker.list_scroll_offset =
                        editor.related_picker.cursor + 1 - visible;
                }
            } else {
                editor.related_picker.cursor = editor.related_picker.cursor.saturating_sub(1);
                if editor.related_picker.cursor < editor.related_picker.list_scroll_offset {
                    editor.related_picker.list_scroll_offset = editor.related_picker.cursor;
                }
            }
        }
        self.schedule_related_picker_preview();
    }

    pub(super) fn related_picker_space_or_toggle(&mut self) {
        match self
            .editor
            .as_ref()
            .map(|editor| editor.related_picker.focus)
        {
            Some(RelatedPickerFocus::Search) => self.insert_related_picker_query_char(' '),
            Some(RelatedPickerFocus::List) => self.toggle_related_picker_selection(),
            None => {}
        }
    }

    pub(super) fn toggle_related_picker_selection(&mut self) {
        let Some(item_id) = self
            .filtered_related_picker_items()
            .get(
                self.editor
                    .as_ref()
                    .map(|editor| editor.related_picker.cursor)
                    .unwrap_or(0),
            )
            .map(|item| item.id)
        else {
            return;
        };
        let Some(editor) = &mut self.editor else {
            return;
        };
        if let Some(index) = editor
            .related_picker
            .selected
            .iter()
            .position(|selected| selected == &item_id)
        {
            editor.related_picker.selected.remove(index);
        } else {
            editor.related_picker.selected.push(item_id);
        }
        mark_editor_dirty(editor);
    }

    pub(super) fn input_related_picker(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        if self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.related_picker.focus == RelatedPickerFocus::List)
        {
            match key.code {
                KeyCode::Char('j') => self.move_related_picker_cursor(true),
                KeyCode::Char('k') => self.move_related_picker_cursor(false),
                _ => {}
            }
            return;
        }
        let Some(editor) = &mut self.editor else {
            return;
        };
        let mut changed = false;
        match key.code {
            KeyCode::Char(ch) => {
                editor
                    .related_picker
                    .query
                    .insert(editor.related_picker.query_cursor, ch);
                editor.related_picker.query_cursor += ch.len_utf8();
                changed = true;
            }
            KeyCode::Backspace => {
                if editor.related_picker.query_cursor > 0 {
                    let previous = prev_boundary(
                        &editor.related_picker.query,
                        editor.related_picker.query_cursor,
                    );
                    editor
                        .related_picker
                        .query
                        .drain(previous..editor.related_picker.query_cursor);
                    editor.related_picker.query_cursor = previous;
                    changed = true;
                }
            }
            KeyCode::Delete => {
                if editor.related_picker.query_cursor < editor.related_picker.query.len() {
                    let next = next_boundary(
                        &editor.related_picker.query,
                        editor.related_picker.query_cursor,
                    );
                    editor
                        .related_picker
                        .query
                        .drain(editor.related_picker.query_cursor..next);
                    changed = true;
                }
            }
            KeyCode::Left => {
                editor.related_picker.query_cursor = prev_boundary(
                    &editor.related_picker.query,
                    editor.related_picker.query_cursor,
                );
            }
            KeyCode::Right => {
                editor.related_picker.query_cursor = next_boundary(
                    &editor.related_picker.query,
                    editor.related_picker.query_cursor,
                );
            }
            KeyCode::Home => editor.related_picker.query_cursor = 0,
            KeyCode::End => editor.related_picker.query_cursor = editor.related_picker.query.len(),
            _ => {}
        }
        if !changed {
            return;
        }
        editor.related_picker.cursor = 0;
        editor.related_picker.list_scroll_offset = 0;
        self.schedule_related_picker_preview();
    }

    pub(super) fn insert_related_picker_query_char(&mut self, ch: char) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor
            .related_picker
            .query
            .insert(editor.related_picker.query_cursor, ch);
        editor.related_picker.query_cursor += ch.len_utf8();
        editor.related_picker.cursor = 0;
        editor.related_picker.list_scroll_offset = 0;
        self.schedule_related_picker_preview();
    }

    pub(super) fn apply_related_picker(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.related = editor.related_picker.selected.clone();
        let display = format_related(&editor.related, &self.all_items);
        editor.set_field_text(EditorField::Related, display);
        editor.related_cursor = editor
            .related_cursor
            .min(editor.related.len().saturating_sub(1));
        mark_editor_dirty(editor);
        self.mode = Mode::Editor;
        self.schedule_selected();
    }

    pub(super) fn open_selected_related_detail(&mut self) {
        let (focus, id_opt) = match &self.editor {
            Some(editor) => (
                editor.focus,
                editor.related.get(editor.related_cursor).copied(),
            ),
            None => return,
        };
        if focus != EditorField::Related {
            return;
        }
        let Some(id) = id_opt else {
            self.open_related_picker();
            return;
        };
        self.push_nav();
        self.open_editor(Some(id));
    }

    pub(super) fn current_related_id(&self) -> Option<EquationId> {
        let editor = self.editor.as_ref()?;
        if editor.focus != EditorField::Related {
            return None;
        }
        editor.related.get(editor.related_cursor).copied()
    }

    pub(super) fn remove_related_from_editor(&mut self, id: EquationId) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.related.retain(|related_id| *related_id != id);
        let display = format_related(&editor.related, &self.all_items);
        editor.set_field_text(EditorField::Related, display);
        editor.related_cursor = editor
            .related_cursor
            .min(editor.related.len().saturating_sub(1));
        mark_editor_dirty(editor);
    }

    pub(super) fn save_editor(&mut self) -> anyhow::Result<()> {
        self.persist_editor(false)
    }

    pub(super) fn copy_current_equation(&mut self) -> anyhow::Result<()> {
        let Some(source_id) = self.selected_id() else {
            return Ok(());
        };
        let source = self.store.get(source_id)?;
        let mut clone = Equation::new(format!("[clone] {}", source.name), source.latex.clone());
        clone.description = source.description;
        clone.assumptions = source.assumptions;
        clone.references = source.references;
        clone.tags = source.tags;
        clone.variables = source.variables;
        clone.related = source.related;
        clone.px_height = source.px_height;
        let clone_id = clone.id;

        self.store.insert_allowing_duplicate_latex(&clone)?;
        self.reload()?;
        if let Some(index) = self.items.iter().position(|item| item.id == clone_id) {
            self.cursor = index;
        }
        self.selected = self.store.get(clone_id).ok();
        self.mode = Mode::Browser;
        self.status = "Equation copied".to_string();
        self.notification = Some(Notification::info("equation copied"));
        self.schedule_selected();
        Ok(())
    }

    pub(super) fn copy_selected_latex_to_clipboard(&mut self) -> anyhow::Result<()> {
        let Some(latex) = self
            .selected
            .as_ref()
            .map(|equation| equation.latex.clone())
        else {
            self.status = "No equation selected".to_string();
            return Ok(());
        };
        crate::clipboard::copy_text(&latex)?;
        self.status = "LaTeX copied to clipboard".to_string();
        self.notification = Some(Notification::info("latex copied"));
        Ok(())
    }

    pub(super) fn open_reference(&mut self) -> anyhow::Result<()> {
        let Some(target) = self.current_reference_target()? else {
            self.status = "No reference URL for selection".to_string();
            return Ok(());
        };
        open_reference_target(&target)?;
        self.status = format!("Opened reference: {target}");
        self.notification = Some(Notification::info("reference opened"));
        Ok(())
    }

    pub(super) fn current_reference_target(&mut self) -> anyhow::Result<Option<String>> {
        if let Some(editor) = self.editor.as_ref()
            && editor.focus == EditorField::References
        {
            let target = self
                .current_reference_index()
                .and_then(|index| editor.references.get(index))
                .and_then(reference_link);
            return Ok(target);
        }

        if let Some(target) = self
            .selected
            .as_ref()
            .and_then(|equation| equation.references.first())
            .and_then(reference_link)
        {
            return Ok(Some(target));
        }

        let Some(id) = self.selected_id() else {
            return Ok(None);
        };
        Ok(self
            .store
            .get(id)?
            .references
            .first()
            .and_then(reference_link))
    }

    pub(super) fn persist_editor(&mut self, exit_after_save: bool) -> anyhow::Result<()> {
        let Some(editor) = &self.editor else {
            return Ok(());
        };
        let fields = editor.field_texts();
        let editing = editor.editing;
        let last_saved_signature = editor.last_saved_signature.clone();
        let related_ids = editor.related.clone();
        let references = editor.references.clone();
        let variables = editor.variables.clone();
        if fields[EditorField::Name.index()].trim().is_empty() {
            return Ok(());
        }
        if fields[EditorField::Latex.index()].trim().is_empty() {
            return Ok(());
        }
        let signature = fields_signature(&fields, &related_ids, &references, &variables);
        if signature == last_saved_signature {
            if let Some(editor) = &mut self.editor {
                editor.dirty = false;
            }
            return Ok(());
        }
        validate_latex(&fields[EditorField::Latex.index()]).map_err(anyhow::Error::msg)?;
        let mut equation = if let Some(id) = editing {
            self.store.get(id)?
        } else {
            let mut equation = Equation::new(
                fields[EditorField::Name.index()].trim().to_string(),
                fields[EditorField::Latex.index()].trim().to_string(),
            );
            equation.px_height = self
                .selected
                .as_ref()
                .map(|selected| selected.px_height)
                .unwrap_or_else(|| self.default_equation_px());
            equation
        };
        equation.name = fields[EditorField::Name.index()].trim().to_string();
        equation.description = fields[EditorField::Description.index()].trim().to_string();
        equation.latex = fields[EditorField::Latex.index()].trim().to_string();
        equation.assumptions = fields[EditorField::Assumptions.index()].trim().to_string();
        equation.references = references.clone();
        equation.tags = {
            let mut seen = std::collections::HashSet::new();
            fields[EditorField::Tags.index()]
                .split(',')
                .map(str::trim)
                .filter(|tag| !tag.is_empty())
                .filter(|tag| seen.insert(*tag))
                .map(ToOwned::to_owned)
                .collect()
        };
        equation.variables = variables.clone();
        equation.related = related_ids;
        equation.updated_at = nullspace_core::store::now_rfc3339();
        let saved_id = equation.id;
        let save_result = if editing.is_some() {
            self.store.update(&equation)
        } else {
            self.store.insert(&equation)
        };
        match save_result {
            Ok(()) => {}
            Err(nullspace_core::Error::Duplicate(_)) => {
                if let Some(editor) = &mut self.editor {
                    editor.dirty = false;
                    editor.last_saved_signature = signature;
                }
                self.status = "An equation with this LaTeX already exists.".to_string();
                return Ok(());
            }
            Err(err) => return Err(err.into()),
        }
        self.reload()?;
        if let Some(index) = self.items.iter().position(|item| item.id == saved_id) {
            self.cursor = index;
        }
        self.selected = self.store.get(saved_id).ok();
        if let Some(editor) = &mut self.editor {
            editor.editing = Some(saved_id);
            editor.dirty = false;
            editor.last_saved_signature = signature;
        }
        self.notification = Some(Notification::info("equation saved"));
        self.status = "Saved".to_string();
        if exit_after_save {
            self.clear_nav();
            self.editor = None;
            self.mode = Mode::Browser;
        }
        self.schedule_selected();
        Ok(())
    }
}

pub(super) fn format_variables(variables: &[Variable]) -> String {
    variables
        .iter()
        .map(|variable| format!("{} = {}", variable.symbol, variable.description))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn quantity_label(quantity: &Quantity) -> String {
    if quantity.name.trim().is_empty() {
        quantity.symbol.clone()
    } else {
        format!("{} - {}", quantity.symbol, quantity.name)
    }
}

pub(super) fn format_related(related: &[EquationId], items: &[EquationSummary]) -> String {
    related
        .iter()
        .filter_map(|id| items.iter().find(|item| item.id == *id))
        .map(|item| item.name.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn fields_signature(
    fields: &[String; EditorField::COUNT],
    related: &[EquationId],
    references: &[Reference],
    variables: &[Variable],
) -> String {
    let related_part = related
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let references_part = references
        .iter()
        .map(|r| {
            format!(
                "{}|{}|{}|{}|{}|{}",
                r.authors,
                r.year.map(|y| y.to_string()).unwrap_or_default(),
                r.title,
                r.doi.clone().unwrap_or_default(),
                r.url.clone().unwrap_or_default(),
                r.pages.clone().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    let variables_part = variables
        .iter()
        .map(|v| {
            format!(
                "{}|{}|{}",
                v.symbol,
                v.description,
                v.quantity_id.map(|id| id.to_string()).unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        fields.join("\u{1f}"),
        related_part,
        references_part,
        variables_part
    )
}

pub(super) fn mark_editor_dirty(editor: &mut EditorState) {
    editor.dirty = true;
    editor.last_change = Instant::now();
}

pub(super) fn related_picker_items_for(
    items: &[EquationSummary],
    editing: Option<EquationId>,
) -> Vec<&EquationSummary> {
    items
        .iter()
        .filter(|item| Some(item.id) != editing)
        .collect()
}

pub(super) fn fuzzy_matches_item(query: &str, item: &EquationSummary) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }
    let needle = query.to_lowercase();
    item.name.to_lowercase().contains(&needle) || item.latex.to_lowercase().contains(&needle)
}
