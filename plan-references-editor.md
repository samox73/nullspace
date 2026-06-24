# Implementation plan: structured reference editor with full citation fields

This is a step-by-step, copy-pasteable plan. Do the steps **in order**. After each
major section, run `cargo check` to catch mistakes early. Run `cargo test`,
`cargo fmt --all`, and `cargo clippy --workspace --all-targets` at the end.

## Background / goal

Today references are typed into a plain text box using a hidden `title | url`
convention (`format_refs`/`parse_refs` in `crates/nullspace-tui/src/app.rs`).
Replace that with:
1. A richer data model: `Reference { authors, year, title, doi, url }`.
2. A structured modal editor (one reference at a time), mirroring the existing
   **Related** field (field index 6) and its picker/confirm modes.
3. DOI normalization + validation.
4. Backward-compatible reading of old `{ "text", "url" }` JSON and old databases.

The **References** field is editor field index **3**. After this change, field 3
is a *list* (like field 6 Related), not a text box.

---

## PART A — core crate (`crates/nullspace-core`)

### A1. New model: `src/model.rs`

Replace the existing `Reference` struct (currently `{ text, url }`) with:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reference {
    #[serde(default)]
    pub authors: String,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(alias = "text", default)]
    pub title: String,
    #[serde(default)]
    pub doi: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}
```

`#[serde(alias = "text")]` makes old JSON (`"text": "..."`) load into `title`.
The `default`s make every field optional on read.

At the bottom of `src/model.rs`, add a test module (or extend an existing one):

```rust
#[cfg(test)]
mod reference_compat_tests {
    use super::Reference;

    #[test]
    fn reference_reads_legacy_text_field() {
        let json = r#"{"text":"Kohn & Sham 1965","url":"https://doi.org/10.1103/PhysRev.140.A1133"}"#;
        let r: Reference = serde_json::from_str(json).unwrap();
        assert_eq!(r.title, "Kohn & Sham 1965");
        assert_eq!(r.url.as_deref(), Some("https://doi.org/10.1103/PhysRev.140.A1133"));
        assert!(r.authors.is_empty());
        assert!(r.year.is_none());
        assert!(r.doi.is_none());
    }
}
```

### A2. New helper module: `src/reference.rs` (create file)

```rust
use crate::model::Reference;

/// Strip common DOI prefixes; return the bare DOI if the input looks like one.
pub fn normalize_doi(input: &str) -> Option<String> {
    let mut s = input.trim();
    for prefix in [
        "https://doi.org/",
        "http://doi.org/",
        "https://dx.doi.org/",
        "http://dx.doi.org/",
        "doi:",
        "DOI:",
    ] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim();
            break;
        }
    }
    if s.starts_with("10.") && s.contains('/') {
        Some(s.to_string())
    } else {
        None
    }
}

/// The clickable link: explicit URL if present, else the DOI as a doi.org URL.
pub fn reference_link(reference: &Reference) -> Option<String> {
    if let Some(url) = reference.url.as_ref() {
        let url = url.trim();
        if !url.is_empty() {
            return Some(url.to_string());
        }
    }
    let doi = reference.doi.as_ref()?.trim();
    if doi.is_empty() {
        None
    } else {
        Some(format!("https://doi.org/{doi}"))
    }
}

/// A single-line human-readable citation for display.
pub fn format_citation(reference: &Reference) -> String {
    let mut out = String::new();
    let authors = reference.authors.trim();
    if !authors.is_empty() {
        out.push_str(authors);
    }
    if let Some(year) = reference.year {
        if out.is_empty() {
            out.push_str(&year.to_string());
        } else {
            out.push_str(&format!(" ({year})"));
        }
    }
    let title = reference.title.trim();
    if !title.is_empty() {
        if out.is_empty() {
            out.push_str(title);
        } else {
            out.push_str(". ");
            out.push_str(title);
        }
    }
    if out.is_empty() {
        out.push_str("(untitled reference)");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Reference;

    fn make(doi: Option<&str>, url: Option<&str>) -> Reference {
        Reference {
            authors: "Kohn, Sham".to_string(),
            year: Some(1965),
            title: "Phys. Rev. 140, A1133".to_string(),
            doi: doi.map(str::to_string),
            url: url.map(str::to_string),
        }
    }

    #[test]
    fn normalize_doi_strips_prefixes() {
        assert_eq!(normalize_doi("10.1103/PhysRev.140.A1133").as_deref(), Some("10.1103/PhysRev.140.A1133"));
        assert_eq!(normalize_doi("https://doi.org/10.1103/X").as_deref(), Some("10.1103/X"));
        assert_eq!(normalize_doi("doi:10.1103/X").as_deref(), Some("10.1103/X"));
        assert_eq!(normalize_doi("not a doi"), None);
        assert_eq!(normalize_doi(""), None);
    }

    #[test]
    fn reference_link_prefers_url_then_doi() {
        assert_eq!(reference_link(&make(Some("10.1/X"), Some("https://x.test"))).as_deref(), Some("https://x.test"));
        assert_eq!(reference_link(&make(Some("10.1/X"), None)).as_deref(), Some("https://doi.org/10.1/X"));
        assert_eq!(reference_link(&make(None, None)), None);
    }

    #[test]
    fn format_citation_combines_fields() {
        assert_eq!(format_citation(&make(None, None)), "Kohn, Sham (1965). Phys. Rev. 140, A1133");
    }
}
```

### A3. Export the module: `src/lib.rs`

Add the module declaration and re-exports (keep the existing lines):

```rust
pub mod reference;
```
and extend the existing `pub use` block with:
```rust
pub use reference::{format_citation, normalize_doi, reference_link};
```

### A4. Store reads/writes: `src/store/mod.rs`

**`load_refs`** — change the SQL and row mapping to the new columns:

```rust
fn load_refs(&self, id: &str) -> Result<Vec<Reference>> {
    let mut stmt = self.conn.prepare(
        "SELECT authors, year, title, doi, url FROM refs WHERE equation_id=?1 ORDER BY position",
    )?;
    let rows = stmt.query_map(params![id], |row| {
        Ok(Reference {
            authors: row.get(0)?,
            year: row.get(1)?,
            title: row.get(2)?,
            doi: row.get(3)?,
            url: row.get(4)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}
```

**`insert_children`** — the `refs` insert loop becomes:

```rust
for (position, reference) in eq.references.iter().enumerate() {
    conn.execute(
        "INSERT INTO refs (equation_id, authors, year, title, doi, url, position)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id,
            reference.authors,
            reference.year,
            reference.title,
            reference.doi,
            reference.url,
            position as i64
        ],
    )?;
}
```

**Test constructor `full_equation`** — update the `eq.references = vec![Reference { .. }]`
literal to the new fields:

```rust
eq.references = vec![Reference {
    authors: "Einstein".to_string(),
    year: Some(1905),
    title: "Annalen der Physik".to_string(),
    doi: None,
    url: Some("https://example.test".to_string()),
}];
```

### A5. Migration: `src/store/migrations.rs`

**(a)** In the `SCHEMA` string constant, replace the `CREATE TABLE IF NOT EXISTS refs (...)`
block with:

```sql
CREATE TABLE IF NOT EXISTS refs (
    equation_id TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
    authors     TEXT NOT NULL DEFAULT '',
    year        INTEGER,
    title       TEXT NOT NULL DEFAULT '',
    doi         TEXT,
    url         TEXT,
    position    INTEGER NOT NULL
);
```

**(b)** In `pub fn migrate(...)`, after the `if version < 3 { migrate_v3(&tx)?; }`
block, add:

```rust
    if version < 4 {
        migrate_v4(&tx)?;
    }
```

**(c)** Add the new migration function (near `migrate_v3`):

```rust
fn migrate_v4(conn: &Connection) -> Result<()> {
    // Old databases have refs(text, url, ...). Rebuild into the citation schema,
    // mapping the old free-text `text` into the new `title` column.
    if column_exists(conn, "refs", "text")? {
        conn.execute_batch(
            r#"
            CREATE TABLE refs_new (
                equation_id TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
                authors     TEXT NOT NULL DEFAULT '',
                year        INTEGER,
                title       TEXT NOT NULL DEFAULT '',
                doi         TEXT,
                url         TEXT,
                position    INTEGER NOT NULL
            );
            INSERT INTO refs_new (equation_id, authors, year, title, doi, url, position)
                SELECT equation_id, '', NULL, text, NULL, url, position FROM refs;
            DROP TABLE refs;
            ALTER TABLE refs_new RENAME TO refs;
            "#,
        )?;
    }
    conn.pragma_update(None, "user_version", 4_i64)?;
    Ok(())
}
```

**(d)** Add a migration test in `src/store/mod.rs` `#[cfg(test)] mod tests` (next to
the existing `migration_dedups_existing_rows_and_preserves_relations` test):

```rust
#[test]
fn migration_v4_upgrades_refs_to_citation_columns() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE equations (
            id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
            latex TEXT NOT NULL, latex_norm TEXT NOT NULL DEFAULT '',
            px_height INTEGER NOT NULL DEFAULT 48,
            created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
            allow_duplicate_latex INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE refs (
            equation_id TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
            text TEXT NOT NULL, url TEXT, position INTEGER NOT NULL
        );
        PRAGMA user_version = 3;
        "#,
    )
    .unwrap();
    let id = EquationId::new().to_string();
    conn.execute(
        "INSERT INTO equations (id, name, description, latex, latex_norm, px_height, created_at, updated_at)
         VALUES (?1, 'n', '', 'x', '', 48, 't', 't')",
        params![id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO refs (equation_id, text, url, position)
         VALUES (?1, 'Kohn & Sham 1965', 'https://doi.org/10.1103/PhysRev.140.A1133', 0)",
        params![id],
    )
    .unwrap();

    migrations::migrate(&conn).unwrap();

    let (title, url, authors): (String, Option<String>, String) = conn
        .query_row(
            "SELECT title, url, authors FROM refs WHERE equation_id=?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(title, "Kohn & Sham 1965");
    assert_eq!(url.as_deref(), Some("https://doi.org/10.1103/PhysRev.140.A1133"));
    assert_eq!(authors, "");
}
```

Run `cargo test -p nullspace-core` now. All core tests should pass.

---

## PART B — TUI crate (`crates/nullspace-tui`)

### B1. `src/app.rs` — imports

Add to the existing `use nullspace_core::{...}` import (it already imports `Reference`):
make sure `Reference` is in the list (it is). Add a separate import line:

```rust
use nullspace_core::reference::normalize_doi;
```

### B2. `src/app.rs` — `Mode` enum

Add two variants:

```rust
pub enum Mode {
    Browser,
    Search,
    Editor,
    RelatedPicker,
    ReferenceEditor,                 // NEW
    ConfirmDelete(EquationId),
    ConfirmRemoveRelated(EquationId),
    ConfirmRemoveReference(usize),   // NEW
}
```

`Mode` derives `Copy`; `usize` is `Copy`, so this is fine.

### B3. `src/app.rs` — `EditorState` + new `ReferenceForm`

Add three fields to `EditorState`:

```rust
pub struct EditorState {
    pub editing: Option<EquationId>,
    pub fields: [TextArea<'static>; 7],
    pub focus: usize,
    pub related_cursor: usize,
    pub reference_cursor: usize,       // NEW
    pub references: Vec<Reference>,    // NEW
    pub reference_form: ReferenceForm, // NEW
    pub dirty: bool,
    pub last_change: Instant,
    pub last_saved_signature: String,
    pub related_picker: RelatedPickerState,
    pub related: Vec<EquationId>,
}
```

Add the new struct + label constant + impl (place near `EditorState`):

```rust
pub const REFERENCE_FIELD_LABELS: [&str; 5] = ["Authors", "Year", "Title", "DOI", "URL"];

#[derive(Clone)]
pub struct ReferenceForm {
    pub fields: [TextArea<'static>; 5], // authors, year, title, doi, url
    pub focus: usize,
    pub editing: Option<usize>,         // index into references; None = adding new
    pub error: Option<String>,
}

impl ReferenceForm {
    fn empty() -> Self {
        Self {
            fields: std::array::from_fn(|_| textarea_from_text("")),
            focus: 0,
            editing: None,
            error: None,
        }
    }

    fn from_reference(reference: &Reference, index: usize) -> Self {
        let values = [
            reference.authors.clone(),
            reference.year.map(|y| y.to_string()).unwrap_or_default(),
            reference.title.clone(),
            reference.doi.clone().unwrap_or_default(),
            reference.url.clone().unwrap_or_default(),
        ];
        Self {
            fields: values.each_ref().map(|v| textarea_from_text(v)),
            focus: 0,
            editing: Some(index),
            error: None,
        }
    }

    fn field_text(&self, index: usize) -> String {
        textarea_text(&self.fields[index])
    }
}
```

### B4. `src/app.rs` — `open_editor`

- Capture references before building the field array; set field index 3 to an empty
  string (it is no longer typed into).
- Add the three new fields to the `EditorState { .. }` construction.
- Update the `fields_signature(...)` call (signature gains a references argument; see B9).

Rewrite `open_editor` body to match:

```rust
fn open_editor(&mut self, id: Option<EquationId>) {
    let equation = id.and_then(|eq_id| self.store.get(eq_id).ok());
    self.selected = equation.clone();
    let initial_related = equation.as_ref().map(|eq| eq.related.clone()).unwrap_or_default();
    let initial_references = equation.as_ref().map(|eq| eq.references.clone()).unwrap_or_default();
    let field_values = if let Some(eq) = equation {
        [
            eq.name,
            eq.description,
            eq.latex,
            String::new(), // field 3 (References) is a list, not a text box
            eq.tags.join(", "),
            format_variables(&eq.variables),
            format_related(&initial_related, &self.all_items),
        ]
    } else {
        [
            String::new(), String::new(), String::new(), String::new(),
            String::new(), String::new(), String::new(),
        ]
    };
    let fields = field_values.each_ref().map(|value| textarea_from_text(value));
    self.editor = Some(EditorState {
        editing: id,
        last_saved_signature: fields_signature(&field_values, &initial_related, &initial_references),
        fields,
        focus: 0,
        related_cursor: 0,
        reference_cursor: 0,
        references: initial_references,
        reference_form: ReferenceForm::empty(),
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
```

### B5. `src/app.rs` — `input_editor` (field 3 keys)

Inside the `match key.code { ... }`, add these arms **alongside** the existing
`focused == 6` arms (before the final two catch-all arms):

```rust
            KeyCode::Char('a') if focused == 3 => {
                self.open_reference_form(None);
                return;
            }
            KeyCode::Char('k') if focused == 3 => {
                editor.reference_cursor = editor.reference_cursor.saturating_sub(1);
                return;
            }
            KeyCode::Char('j') if focused == 3 => {
                let max = editor.references.len().saturating_sub(1);
                editor.reference_cursor = (editor.reference_cursor + 1).min(max);
                return;
            }
            KeyCode::Char('d') if focused == 3 => {
                if let Some(idx) = self.current_reference_index() {
                    self.mode = Mode::ConfirmRemoveReference(idx);
                }
                return;
            }
            KeyCode::Enter if focused == 3 => {
                if let Some(idx) = self.current_reference_index() {
                    self.open_reference_form(Some(idx));
                }
                return;
            }
```

Then change the existing catch-all text-input arm to also skip field 3:

```rust
            _ if focused != 6 && focused != 3 && editor.fields[focused].input(key) => {}
```

### B6. `src/app.rs` — new helper + form methods

Add these methods inside `impl AppState` (place them near `open_related_picker` /
`current_related_id` for consistency):

```rust
fn current_reference_index(&self) -> Option<usize> {
    let editor = self.editor.as_ref()?;
    if editor.focus != 3 || editor.references.is_empty() {
        return None;
    }
    Some(editor.reference_cursor.min(editor.references.len() - 1))
}

fn open_reference_form(&mut self, index: Option<usize>) {
    let form = {
        let Some(editor) = self.editor.as_ref() else { return };
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

fn input_reference_form(&mut self, key: crossterm::event::KeyEvent) {
    if let Some(editor) = &mut self.editor {
        let focus = editor.reference_form.focus;
        editor.reference_form.fields[focus].input(key);
        editor.reference_form.error = None;
    }
}

fn reference_form_next_field(&mut self) {
    if let Some(editor) = &mut self.editor {
        let n = editor.reference_form.fields.len();
        editor.reference_form.focus = (editor.reference_form.focus + 1) % n;
    }
}

fn reference_form_prev_field(&mut self) {
    if let Some(editor) = &mut self.editor {
        let n = editor.reference_form.fields.len();
        editor.reference_form.focus = (editor.reference_form.focus + n - 1) % n;
    }
}

fn save_reference_form(&mut self) {
    let Some(editor) = &mut self.editor else { return };
    let authors = editor.reference_form.field_text(0).trim().to_string();
    let year_raw = editor.reference_form.field_text(1).trim().to_string();
    let title = editor.reference_form.field_text(2).trim().to_string();
    let doi_raw = editor.reference_form.field_text(3).trim().to_string();
    let url_raw = editor.reference_form.field_text(4).trim().to_string();

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

    // Prefer an explicit DOI; if the DOI box holds something non-DOI keep it as-is.
    // If the DOI box is empty but the URL box holds a DOI, move it into `doi`.
    let (doi, url) = if !doi_raw.is_empty() {
        let doi = normalize_doi(&doi_raw).unwrap_or(doi_raw);
        let url = (!url_raw.is_empty()).then_some(url_raw);
        (Some(doi), url)
    } else if let Some(doi) = normalize_doi(&url_raw) {
        (Some(doi), None)
    } else {
        (None, (!url_raw.is_empty()).then_some(url_raw))
    };

    let reference = Reference { authors, year, title, doi, url };
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

fn remove_reference(&mut self, index: usize) {
    if let Some(editor) = &mut self.editor {
        if index < editor.references.len() {
            editor.references.remove(index);
            editor.reference_cursor =
                editor.reference_cursor.min(editor.references.len().saturating_sub(1));
            mark_editor_dirty(editor);
        }
    }
}
```

> Note on the borrow checker: the tail `mark_editor_dirty(editor); self.mode = Mode::Editor;`
> is the same pattern already used at the end of `apply_related_picker`, so it compiles.

### B7. `src/app.rs` — wire actions in `apply`

In the big `match action { ... }`, add arms (anywhere before `Action::None`):

```rust
            Action::ReferenceEditorNextField => {
                self.reference_form_next_field();
                Ok(())
            }
            Action::ReferenceEditorPrevField => {
                self.reference_form_prev_field();
                Ok(())
            }
            Action::ReferenceEditorSave => {
                self.save_reference_form();
                Ok(())
            }
            Action::ReferenceEditorCancel => {
                self.mode = Mode::Editor;
                Ok(())
            }
            Action::ReferenceEditorInput(key) => {
                self.input_reference_form(key);
                Ok(())
            }
            Action::ConfirmReferenceRemoveYes => {
                if let Mode::ConfirmRemoveReference(index) = self.mode {
                    self.remove_reference(index);
                    self.mode = Mode::Editor;
                }
                Ok(())
            }
            Action::ConfirmReferenceRemoveNo => {
                self.mode = Mode::Editor;
                Ok(())
            }
```

### B8. `src/app.rs` — extend Up/Down + cursor-move handlers for field 3

In `apply`, the `Action::EditorRelatedMoveUp` / `EditorRelatedMoveDown` arms currently
branch on `focus == 6`. Make them also handle `focus == 3`:

```rust
            Action::EditorRelatedMoveUp => {
                if let Some(editor) = &mut self.editor {
                    match editor.focus {
                        6 => editor.related_cursor = editor.related_cursor.saturating_sub(1),
                        3 => editor.reference_cursor = editor.reference_cursor.saturating_sub(1),
                        f => editor.fields[f].move_cursor(CursorMove::Up),
                    }
                }
                Ok(())
            }
            Action::EditorRelatedMoveDown => {
                if let Some(editor) = &mut self.editor {
                    match editor.focus {
                        6 => {
                            let max = editor.related.len().saturating_sub(1);
                            editor.related_cursor = (editor.related_cursor + 1).min(max);
                        }
                        3 => {
                            let max = editor.references.len().saturating_sub(1);
                            editor.reference_cursor = (editor.reference_cursor + 1).min(max);
                        }
                        f => editor.fields[f].move_cursor(CursorMove::Down),
                    }
                }
                Ok(())
            }
```

In the `EditorMoveLeft`, `EditorMoveRight`, `EditorHome`, `EditorEnd` arms, the guard
is currently `if editor.focus != 6`. Change each to `if editor.focus != 6 && editor.focus != 3`
(field 3 has no text cursor to move).

### B9. `src/app.rs` — persistence + signature + delete dead code

**`persist_editor`**: near the top where it reads `let related_ids = editor.related.clone();`,
add `let references = editor.references.clone();`. Then:
- Replace `equation.references = parse_refs(&fields[3]);` with `equation.references = references.clone();`
- Replace both `fields_signature(&fields, &related_ids)` calls with
  `fields_signature(&fields, &related_ids, &references)`.

**`fields_signature`**: change its signature and body to include references:

```rust
fn fields_signature(fields: &[String; 7], related: &[EquationId], references: &[Reference]) -> String {
    let related_part = related.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
    let references_part = references
        .iter()
        .map(|r| {
            format!(
                "{}|{}|{}|{}|{}",
                r.authors,
                r.year.map(|y| y.to_string()).unwrap_or_default(),
                r.title,
                r.doi.clone().unwrap_or_default(),
                r.url.clone().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    format!("{}\u{1f}{}\u{1f}{}", fields.join("\u{1f}"), related_part, references_part)
}
```

**Delete** the now-unused functions `format_refs` and `parse_refs` entirely.

### B10. `src/app.rs` — include new modes in "in editor" checks

Search `app.rs` for `Mode::RelatedPicker | Mode::ConfirmRemoveRelated(_)`. There are a
couple of `matches!(self.mode, ...)` expressions (in `schedule_selected` and
`schedule_latex`) that detect "we are inside the editor". Add the two new modes to each:

```rust
matches!(
    self.mode,
    Mode::Editor | Mode::RelatedPicker | Mode::ConfirmRemoveRelated(_)
        | Mode::ReferenceEditor | Mode::ConfirmRemoveReference(_)
)
```

**`Back` action handler**: the `match self.mode { ... }` inside `Action::Back` must stay
exhaustive. Add arms:
```rust
                    Mode::ReferenceEditor => Mode::Editor,
                    Mode::ConfirmRemoveReference(_) => Mode::Editor,
```
(Esc routes to dedicated cancel actions for these modes, so these arms are just for
exhaustiveness / safety.)

### B11. `src/action.rs`

Add these variants to `enum Action`:

```rust
    ReferenceEditorNextField,
    ReferenceEditorPrevField,
    ReferenceEditorSave,
    ReferenceEditorCancel,
    ReferenceEditorInput(crossterm::event::KeyEvent),
    ConfirmReferenceRemoveYes,
    ConfirmReferenceRemoveNo,
```

### B12. `src/event.rs`

Add two new match arms in `map_key`'s `match mode { ... }`:

```rust
        Mode::ReferenceEditor => {
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
                return Action::ReferenceEditorSave;
            }
            match key.code {
                KeyCode::Esc => Action::ReferenceEditorCancel,
                KeyCode::Enter => Action::ReferenceEditorSave,
                KeyCode::Tab => Action::ReferenceEditorNextField,
                KeyCode::BackTab => Action::ReferenceEditorPrevField,
                _ => Action::ReferenceEditorInput(key),
            }
        }
        Mode::ConfirmRemoveReference(_) => match key.code {
            KeyCode::Char('y') | KeyCode::Char('d') | KeyCode::Enter => Action::ConfirmReferenceRemoveYes,
            KeyCode::Char('n') | KeyCode::Esc => Action::ConfirmReferenceRemoveNo,
            _ => Action::None,
        },
```

(The `Mode::Editor` arm needs no change: Up/Down already map to
`EditorRelatedMoveUp/Down`, which B8 extended for field 3.)

### B13. `src/ui/mod.rs`

In the `match app.mode` dispatch, add the new modes to the **editor** branch so it stays
exhaustive:

```rust
        Mode::Editor | Mode::RelatedPicker | Mode::ConfirmRemoveRelated(_)
        | Mode::ReferenceEditor | Mode::ConfirmRemoveReference(_) => editor::draw(frame, app),
```

### B14. `src/ui/widgets.rs` — `status_bar`

The `let help = match app.mode { ... }` must stay exhaustive. Add:

```rust
        Mode::ReferenceEditor => "tab/shift-tab field  enter save  esc cancel",
        Mode::ConfirmRemoveReference(_) => "y/enter remove  n/esc cancel",
```

### B15. `src/ui/editor.rs` — render the list field + modal + confirm

**(a)** Field title: where the code computes the per-row `title` (currently special-cases
`index == 6`), special-case index 3 too:

```rust
            let title = match index {
                3 => "References (a add, enter edit, d remove)",
                6 => "Related (up/down select, enter open, r edit)",
                _ => LABELS[index],
            };
```

**(b)** In the per-row render loop, where it does `if index == 6 { render_related_field(...); continue; }`,
add an analogous block for index 3 **before** the textarea rendering:

```rust
            if index == 3 {
                render_reference_field(frame, *area, block, &editor.references, editor.reference_cursor);
                continue;
            }
```

**(c)** Row heights: remove `3` from `MULTILINE_FIELDS` (becomes `[1, 2, 5]`). In
`editor_row_constraints`, add an explicit arm for index 3:

```rust
            3 => Constraint::Length(reference_box_height(editor)),
```
and add the helper:
```rust
fn reference_box_height(editor: &crate::app::EditorState) -> u16 {
    let content = (editor.references.len() as u16).saturating_mul(2).max(2); // 2 lines/ref, min 2
    (content + BLOCK_CHROME_ROWS).min(MAX_TEXT_BOX_LINES + BLOCK_CHROME_ROWS)
}
```

**(d)** Add the field renderer (mirror `render_related_field`):

```rust
fn render_reference_field(
    frame: &mut Frame<'_>,
    area: Rect,
    block: Block<'_>,
    references: &[nullspace_core::Reference],
    cursor: usize,
) {
    if references.is_empty() {
        frame.render_widget(
            Paragraph::new("No references\n\nPress a to add one (title, authors, year, DOI/URL)")
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
            ListItem::new(vec![
                Line::from(citation),
                Line::styled(link, Style::default().fg(Color::DarkGray)),
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
```

**(e)** Add the modal editor (mirror `draw_related_picker`). Note it takes `&mut AppState`
because it sets blocks on the form's text areas:

```rust
fn draw_reference_editor(frame: &mut Frame<'_>, app: &mut AppState) {
    let Some(editor) = &mut app.editor else { return };
    let area = centered_rect(70, 19, frame.area());
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
            Constraint::Length(1),
        ])
        .split(inner);

    for index in 0..5 {
        let focused = editor.reference_form.focus == index;
        let style = if focused {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
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
            "tab next · shift-tab prev · enter save · esc cancel",
            Style::default().fg(Color::DarkGray),
        ),
    };
    frame.render_widget(Paragraph::new(hint), rows[5]);
}
```

**(f)** Add the confirm dialog (mirror `draw_remove_related_confirm`):

```rust
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
```

**(g)** Hook the modals into `pub fn draw(...)`. Where it currently draws the related
picker / confirm-remove-related (after `widgets::preview_pane(...)`, before
`widgets::status_bar(...)`), add:

```rust
    if matches!(app.mode, Mode::ReferenceEditor) {
        draw_reference_editor(frame, app);
    }
    if let Mode::ConfirmRemoveReference(index) = app.mode {
        draw_remove_reference_confirm(frame, app, index);
    }
```

Ensure the `use` block at the top of `editor.rs` already imports the items used above
(`Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap`, `Line`,
`Color, Modifier, Style`, `Constraint, Direction, Layout, Rect`, `Frame`). They are
already imported for the existing related-picker code.

---

## PART C — docs

### C1. `README.md`

In the **Editor** section, document the References field. Add a short block, e.g.:

```
In the **References** field: `a` to add a reference, `Enter` to edit the highlighted
one, `d` to remove it, `j`/`k` (or ↑/↓) to move. Each reference has authors, year,
title, DOI, and URL fields; a bare DOI (e.g. `10.1103/PhysRev.140.A1133`) is
automatically turned into a `https://doi.org/…` link. Existing libraries and JSON
files using the old single-line reference format still import unchanged.
```

---

## PART D — verification

Run, in order:

1. `cargo test` — everything green. Key new tests: `reference_reads_legacy_text_field`,
   the `reference` module unit tests, `migration_v4_upgrades_refs_to_citation_columns`.
2. Legacy import still works:
   ```sh
   NULLSPACE_DB=/tmp/refs.sqlite3 cargo run -p nullspace-tui -- --import dft-diagmc.json
   NULLSPACE_DB=/tmp/refs.sqlite3 cargo run -p nullspace-tui
   ```
   Open an equation that has references (e.g. "Kohn-Sham equations") → the References
   field lists the citation with its DOI link.
3. Manual editor flow (same temp DB):
   - Open an equation, `Tab` to **References**.
   - `a` → modal opens; `Tab` between fields; type a bare DOI `10.1103/PhysRev.136.B864`
     in the DOI box; `Enter` → list shows the new reference with a `doi.org` link.
   - `Enter` on a reference → edit it; change the year to a non-number → `Enter` shows
     "Year must be a number"; fix it → saves.
   - `d` on a reference → confirm dialog → `y` removes it.
   - `Esc` out of the editor; reopen → references persisted.
4. `cargo fmt --all` then `cargo clippy --workspace --all-targets` — no warnings.

## Notes / gotchas

- `Mode` is `Copy`; keep it that way (`ConfirmRemoveReference(usize)` is fine).
- Several `match app.mode` / `match self.mode` expressions are exhaustive — adding the
  two new `Mode` variants will cause compile errors anywhere a match isn't updated.
  The compiler will point you at each one (apply's `Back`, `ui/mod.rs`, `widgets.rs`
  `status_bar`). Add the arms shown above.
- Do **not** type into field 3 as text anymore — that's why the `input_editor`
  catch-all gains `&& focused != 3`, and the cursor-move handlers gain `&& editor.focus != 3`.
- Old JSON exports remain importable because of `#[serde(alias = "text")]` + `default`.
  New exports emit the full citation fields.
```
