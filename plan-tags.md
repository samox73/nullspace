# Plan: Tag browsing & filtering

Two user-facing capabilities:

1. A **`tags` picker** — a modal listing every tag alphabetically with its item
   count (`35 diagmc`, ` 8 dft`), selectable; selecting a tag filters the browser
   to exactly that tag.
2. **Untagged filter** — surfaced as a pinned pseudo-row (`12 (untagged)`) inside
   the same picker rather than a separate command.

Decisions locked in: **Option B** (a dedicated `BrowserFilter::Tag(String)` with
*exact* match semantics, not the substring `tag:` search) and **picker row** for
untagged (no standalone command). Single-select for v1.

The tag picker is a new modal mode that mirrors `Mode::Trash` almost exactly
(own cursor/scroll state, full-screen list, `status_bar` at the bottom, reached
via a command, exited with `esc`/`q`).

---

## 1. Store layer — `crates/nullspace-core/src/store/mod.rs`

The scoped `tag:` search (`search_scoped` / `SearchScope::Tag`, line ~100) is a
substring `LIKE` match and stays as-is for the typed search box. The picker needs
**exact** match plus an **untagged** query and an **untagged count**.

### 1a. `by_tag` — exact-match filter

Mirror `by_symbol` (line ~121). Exact, case-insensitive on the stored tag:

```rust
pub fn by_tag(&self, tag: &str) -> Result<Vec<EquationSummary>> {
    let tag = tag.trim();
    if tag.is_empty() {
        return self.list();
    }
    let mut stmt = self.conn.prepare(
        "SELECT DISTINCT e.id, e.name, e.description, e.latex, e.px_height
         FROM equations e
         JOIN tags t ON t.equation_id = e.id
         WHERE lower(t.tag) = lower(?1)
         ORDER BY e.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map(params![tag], summary_from_row)?;
    collect_summaries(rows)
}
```

### 1b. `untagged` — equations with no tags

```rust
pub fn untagged(&self) -> Result<Vec<EquationSummary>> {
    let mut stmt = self.conn.prepare(
        "SELECT e.id, e.name, e.description, e.latex, e.px_height
         FROM equations e
         LEFT JOIN tags t ON t.equation_id = e.id
         WHERE t.equation_id IS NULL
         ORDER BY e.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], summary_from_row)?;
    collect_summaries(rows)
}
```

### 1c. `untagged_count` — for the pseudo-row

Cheap dedicated count so the picker row can show a number without materializing
the full list:

```rust
pub fn untagged_count(&self) -> Result<usize> {
    let count: i64 = self.conn.query_row(
        "SELECT COUNT(*) FROM equations e
         WHERE NOT EXISTS (SELECT 1 FROM tags t WHERE t.equation_id = e.id)",
        [],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}
```

`tag_counts()` (line ~79) is left untouched — it stays frequency-sorted because
`browser.rs::search_details` relies on that ordering for typed `tag:` suggestions.
Alphabetical ordering for the picker is done in the view layer (§4b).

### Store tests (append to existing `#[cfg(test)] mod tests`)
- `by_tag_matches_exactly` — `by_tag("dft")` does **not** return a `dft-plus-u`
  tagged item (guards the exact-vs-substring decision).
- `by_tag_is_case_insensitive`.
- `untagged_returns_only_equations_without_tags`.
- `untagged_count_matches_untagged_len`.

---

## 2. Filter model — `crates/nullspace-tui/src/app.rs`

### 2a. Extend `BrowserFilter` (line ~156)

```rust
pub enum BrowserFilter {
    None,
    Search(String),
    Tag(String),
    Untagged,
}
```

### 2b. `refresh_items` (line ~411) — add arms

```rust
self.items = match &self.browser_filter {
    BrowserFilter::None => self.all_items.clone(),
    BrowserFilter::Search(query) => self.store.search(query)?,
    BrowserFilter::Tag(tag) => self.store.by_tag(tag)?,
    BrowserFilter::Untagged => self.store.untagged()?,
};
```

### 2c. `browser_title` (line ~333) — add arms

```rust
BrowserFilter::Tag(tag) => format!("Tag: {tag}"),
BrowserFilter::Untagged => "Untagged".to_string(),
```

`clear_browser_filter` already resets to `None`, so `esc` in the browser clears a
tag/untagged filter with no change. The browser's `esc → ClearFilter` binding
already covers exit.

---

## 3. New mode, state & actions

### 3a. `Mode::TagPicker` — `app.rs` `Mode` enum (line ~42)

Add `TagPicker` variant.

### 3b. `AppState` fields (near `trash_cursor`, line ~181)

```rust
pub tag_picker_cursor: usize,
pub tag_picker_scroll_offset: usize,
pub tag_picker_visible_height: u16,
pub untagged_count: usize,
```

Initialize all to `0` in `AppState::open` (line ~250).

### 3c. Keep `untagged_count` fresh in `reload` (line ~308)

Alongside `self.tag_counts = self.store.tag_counts()?;` add:

```rust
self.untagged_count = self.store.untagged_count()?;
```

### 3d. Picker row model + builder

A row is either a real tag or the untagged pseudo-row. Define near the other
small helper types:

```rust
pub enum TagPickerRow {
    Untagged { count: usize },
    Tag { name: String, count: usize },
}
```

Builder on `AppState` — untagged pinned first (only when non-empty), tags sorted
case-insensitively alphabetical:

```rust
pub fn tag_picker_rows(&self) -> Vec<TagPickerRow> {
    let mut rows = Vec::new();
    if self.untagged_count > 0 {
        rows.push(TagPickerRow::Untagged { count: self.untagged_count });
    }
    let mut tags = self.tag_counts.clone();
    tags.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    rows.extend(tags.into_iter().map(|(name, count)| TagPickerRow::Tag { name, count }));
    rows
}
```

### 3e. New `Action` variants — `crates/nullspace-tui/src/action.rs`

```rust
OpenTags,
TagPickerMoveUp,
TagPickerMoveDown,
TagPickerMoveToTop,
TagPickerMoveToBottom,
TagPickerApply,
TagPickerCancel,
```

### 3f. `apply` arms — `app.rs` (in the big match, near the Trash arms)

- `OpenTags`: `self.tag_picker_cursor = 0; self.tag_picker_scroll_offset = 0;
  self.mode = Mode::TagPicker;` set a status like
  `format!("Tags: {} tag(s)", self.tag_counts.len())`. (Counts are already current
  from the last `reload`; no extra query needed here.)
- `TagPickerMoveUp/Down/ToTop/ToBottom`: clamp `tag_picker_cursor` against
  `self.tag_picker_rows().len()`, mirroring the Trash cursor arms (and the
  scroll-offset bookkeeping used by `move_browser_cursor_to`). A small
  `move_tag_picker_cursor_to(usize)` helper keeps scroll math in one place.
- `TagPickerApply`: read the row at the cursor and set the filter:

```rust
let rows = self.tag_picker_rows();
if let Some(row) = rows.get(self.tag_picker_cursor) {
    self.browser_filter = match row {
        TagPickerRow::Untagged { .. } => BrowserFilter::Untagged,
        TagPickerRow::Tag { name, .. } => BrowserFilter::Tag(name.clone()),
    };
    self.cursor = 0;
    self.refresh_items()?;
    self.status = format!("{}: {} match(es)", self.browser_title(), self.items.len());
}
self.mode = Mode::Browser;
self.schedule_selected();
```

- `TagPickerCancel`: `self.mode = Mode::Browser;` (no filter change).

### 3g. Register the `tags` command (`app.rs` ~2399)

```rust
const COMMANDS: [&str; 6] = ["delete", "exit", "new", "search", "tags", "trash"];
```

`command_action` (line ~2425): add `"tags" => Some(Action::OpenTags),`.
(`execute_cmdline` already closes the cmdline, sets `Mode::Browser`, and calls
`force_preview_redraw` before dispatching the action, so the picker opens cleanly
over a redrawn frame.)

---

## 4. Rendering

### 4a. Draw dispatch — `crates/nullspace-tui/src/ui/mod.rs` (line ~14)

Add a `Mode::TagPicker` arm next to the Trash arm:

```rust
Mode::TagPicker => {
    widgets::clear_cmdline_overlay(frame, cmdline_area);
    widgets::tag_picker(frame, app);
}
```

### 4b. `tag_picker` widget — `crates/nullspace-tui/src/ui/widgets.rs`

Model on `trash` / `trash_list` (line ~443). Outer `Layout` with `Min(1)` list +
`Length(1)` status bar. Rows from `app.tag_picker_rows()`:

- Label per row: `format!("{count:>3} {label}")` where `label` is the tag name or
  `(untagged)`. This matches the existing `search_details` formatting
  (`browser.rs:113`) and the spec's right-aligned count column.
- Style the `(untagged)` row dimmer (e.g. `Color::DarkGray`) to read as special.
- `List` with title `"Tags"`, `highlight_style` bg `DarkGray`, `highlight_symbol
  "> "`, driven by a `ListState` seeded with `app.tag_picker_cursor`.
- Empty state (`rows.is_empty()`, i.e. no tags and nothing untagged): a centered
  `"No tags"` paragraph, exactly like the empty-Trash branch.
- Set `app.tag_picker_visible_height` from the drawn area height if scroll handling
  needs it (only required once the list can exceed the viewport; the Trash list
  currently relies on `ListState` auto-scroll, so matching that is fine for v1 and
  the explicit scroll-offset field can be deferred).

---

## 5. Key routing — `crates/nullspace-tui/src/event.rs`

Add a `Mode::TagPicker` arm mirroring `Mode::Trash` (line ~95):

```rust
Mode::TagPicker => match key.code {
    KeyCode::Char('j') | KeyCode::Down => Action::TagPickerMoveDown,
    KeyCode::Char('k') | KeyCode::Up => Action::TagPickerMoveUp,
    KeyCode::Char('g') if app.vim_go_prefix => Action::TagPickerMoveToTop,
    KeyCode::Char('g') => Action::StartGoPrefix,
    KeyCode::Char('G') => Action::TagPickerMoveToBottom,
    KeyCode::Enter => Action::TagPickerApply,
    KeyCode::Esc | KeyCode::Char('q') => Action::TagPickerCancel,
    _ => Action::None,
},
```

---

## 6. Help & discoverability

- Add a `tags` line to the `help_modal` command list (`widgets.rs` ~334) so the
  command is discoverable: e.g. `tags   browse & filter by tag`.
- Optional follow-up (not v1): a direct browser keybinding (e.g. `t`) → `OpenTags`.
  Left out for now to keep the surface to the command, per the decision.

---

## 7. Test plan

**Store (`store/mod.rs`)** — see §1c.

**App logic (`app.rs` tests)**
- `tag_picker_rows_sorted_alphabetically_with_untagged_first`.
- `tag_picker_rows_omits_untagged_when_zero`.
- `tag_picker_apply_sets_exact_tag_filter` — apply on a tag row ⇒
  `BrowserFilter::Tag`, and `items` equals `store.by_tag(..)`.
- `tag_picker_apply_untagged_row_sets_untagged_filter`.
- `tag_picker_cancel_leaves_filter_untouched`.
- `open_tags_command_enters_tag_picker_mode` (drive `command_action("tags")`).
- `browser_title_reflects_tag_and_untagged_filters`.

**Event (`event.rs` if it has tests, else covered via app)** — `esc`/`q` ⇒
cancel, `enter` ⇒ apply, `j/k` ⇒ move.

---

## 8. Implementation order (checklist)

1. Store: `by_tag`, `untagged`, `untagged_count` + tests. ✅ compiles & tests green.
2. `BrowserFilter::{Tag, Untagged}` + `refresh_items` / `browser_title` arms.
3. `Mode::TagPicker`, `AppState` fields, `reload` wiring, `tag_picker_rows`.
4. `Action` variants + `apply` arms + `move_tag_picker_cursor_to` helper.
5. `tags` in `COMMANDS` + `command_action`.
6. `event.rs` routing arm.
7. `widgets::tag_picker` + `ui/mod.rs` dispatch.
8. Help text.
9. App-level tests.
10. `cargo fmt`, `cargo clippy`, `cargo test`; manual smoke: `:tags` → arrow →
    enter filters; `(untagged)` row filters; `esc` in browser clears.

## Files touched
- `crates/nullspace-core/src/store/mod.rs` — `by_tag`, `untagged`, `untagged_count`, tests.
- `crates/nullspace-tui/src/action.rs` — 7 new `Action` variants.
- `crates/nullspace-tui/src/app.rs` — filter enum, mode, state, rows builder, apply arms, command wiring.
- `crates/nullspace-tui/src/event.rs` — `Mode::TagPicker` key routing.
- `crates/nullspace-tui/src/ui/mod.rs` — draw dispatch.
- `crates/nullspace-tui/src/ui/widgets.rs` — `tag_picker` widget, help text.

## Out of scope (future)
- Multi-select / AND-OR tag combinations.
- Persistent tag sidebar (vs modal).
- A browser keybinding for the picker.
- Tag rename/merge from the picker.
