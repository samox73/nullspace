# Plan: Trash (soft-delete with restore)

## Goal

Deleting an equation should not destroy it. Instead it moves to a **trash**,
from which it can be **restored** or **permanently deleted**. A new in-app
cmdline command `trash` opens a view listing trashed equations; in that view
`r`/`restore` brings an entry back and `d`/`delete` purges it for good.

Scope is deletes only — edits continue to overwrite in place with no history.

## Design decision: separate `trash` table with a JSON snapshot

We add one append-only table that stores a full serialized snapshot of the
equation (including its children), rather than a `deleted` flag on `equations`.

Rationale (see discussion):

- The partial unique index on `latex_norm` (`WHERE allow_duplicate_latex = 0`)
  would otherwise keep a trashed equation's identity reserved, blocking
  re-creation of the same formula until purge. A separate table avoids
  re-scoping that index and the dedup paths (`load_existing_by_norm`, import).
- All live read paths (`list`, `search`, `search_scoped`, `by_symbol`, `all`,
  `get`) stay unchanged — no `WHERE deleted = 0` sprinkled everywhere.
- Children (`variables`, `tags`, `refs`, `related`) cascade away on delete
  exactly as today; the snapshot blob captures them for restore.

`Equation` already derives `Serialize`/`Deserialize`, so `serde_json` gives us
the snapshot for free.

## Data model

### Migration v5 (`store/migrations.rs`)

```sql
CREATE TABLE IF NOT EXISTS trash (
    id          TEXT PRIMARY KEY,   -- original equation id (UUID)
    name        TEXT NOT NULL,      -- denormalized for the list view
    snapshot    TEXT NOT NULL,      -- serde_json of the full Equation
    deleted_at  TEXT NOT NULL       -- rfc3339, when it was trashed
);
```

- Add the table to the `SCHEMA` constant (for fresh DBs) **and** add a
  `migrate_v5` that `CREATE TABLE IF NOT EXISTS trash (...)` and bumps
  `user_version` to 5. Wire `if version < 5 { migrate_v5(&tx)?; }` into
  `migrate`.
- No FK from `trash` to `equations` (the live row is gone once trashed).

### Model (`model.rs`)

Add a lightweight summary used by the trash view (mirrors `EquationSummary`):

```rust
#[derive(Debug, Clone)]
pub struct TrashEntry {
    pub id: EquationId,
    pub name: String,
    pub deleted_at: String,
}
```

## Store layer (`store/mod.rs`)

Replace the current hard-delete semantics for the TUI path and add restore/
purge/list. Concretely:

- **`trash(&mut self, id: EquationId) -> Result<()>`**
  1. `let eq = self.get(id)?;` (errors `NotFound` if absent)
  2. serialize: `serde_json::to_string(&eq)`
  3. in a transaction: `INSERT INTO trash (id, name, snapshot, deleted_at)`
     then `DELETE FROM equations WHERE id=?1` (children cascade as today)
  4. commit
- **`restore(&mut self, id: EquationId) -> Result<()>`**
  1. read `snapshot` from `trash` (errors `NotFound` if absent)
  2. `serde_json::from_str::<Equation>`
  3. re-insert via the existing insert path (`insert_equation_row` +
     `insert_children`) inside a transaction, then
     `DELETE FROM trash WHERE id=?1`, then commit.
  4. **Conflict handling:** if a new equation now occupies the same
     `latex_norm`, the insert raises `Error::Duplicate`. Propagate it; the TUI
     surfaces "Can't restore: a conflicting equation exists" and the entry
     stays in trash (transaction rolls back).
  5. **Dangling `related` edges:** the snapshot may reference equations that no
     longer exist. `insert_children` uses `INSERT OR IGNORE`, which SQLite
     silently skips on FK violation, so stale edges drop cleanly. (Reverse
     edges that other equations had pointing at this one were cascade-deleted
     at trash time and are not resurrected — acceptable, note in a comment.)
- **`purge(&mut self, id: EquationId) -> Result<()>`**
  `DELETE FROM trash WHERE id=?1`. (Idempotent; no error if already gone, or
  return `NotFound` — pick `NotFound` for symmetry with `restore`.)
- **`list_trash(&self) -> Result<Vec<TrashEntry>>`**
  `SELECT id, name, deleted_at FROM trash ORDER BY deleted_at DESC`.

Keep the existing **`delete(&mut self, id)`** as the raw hard-delete (used by
internal/import paths and existing tests like `delete_cascades`); the TUI stops
calling it directly.

### Add `serde_json` dependency

Confirm `serde_json` is available to `nullspace-core` (add to its `Cargo.toml`
if not; `serde` is already a dep).

## TUI layer

### Command registry (`app.rs`)

- `COMMANDS`: add `"trash"` → `const COMMANDS: [&str; 5] = ["delete", "exit",
  "new", "search", "trash"];` (keep alphabetical so the existing
  `command_matches` ordering tests stay predictable — update those asserts).
- `command_action`: `"trash" => Some(Action::OpenTrash)`.

### Mode + state (`app.rs`)

- `Mode` enum: add `Trash`, and `ConfirmPurge(EquationId)` (purge is
  irreversible, so confirm it — reuse the `ConfirmDelete` pattern).
- `AppState`: add `trash_items: Vec<TrashEntry>` and `trash_cursor: usize`;
  initialize both empty/0 in the constructor.

### Actions (`action.rs`)

Add: `OpenTrash`, `TrashMoveUp`, `TrashMoveDown`, `TrashRestore`,
`TrashPurgeRequest`, `Back` already exists for leaving the view.
`ConfirmPurge` reuses new `ConfirmPurgeYes`/`ConfirmPurgeNo` (or generalize
`ConfirmYes`/`ConfirmNo` — but those are matched against `Mode::ConfirmDelete`,
so add dedicated variants to keep the match arms unambiguous).

### Delete flow change (`app.rs`, `Action::ConfirmYes`)

In the `Mode::ConfirmDelete(id)` branch, change `self.store.delete(id)?` to
`self.store.trash(id)?`; status becomes `"Moved to trash"`.

### Event mapping (`event.rs`)

- Add a `Mode::Trash` arm:
  - `j`/Down → `TrashMoveDown`, `k`/Up → `TrashMoveUp`
  - `r` → `TrashRestore`
  - `d`/Delete → `TrashPurgeRequest`
  - `Esc`/`q` → `Back`
- Add a `Mode::ConfirmPurge(_)` arm mirroring `ConfirmDelete`
  (`y`/`d`/Enter → yes, `n`/Esc → no).

### Action handling (`app.rs` apply)

- `OpenTrash`: `self.trash_items = self.store.list_trash()?; self.trash_cursor =
  0; self.mode = Mode::Trash;`
- `TrashMoveUp`/`TrashMoveDown`: clamp cursor against `trash_items.len()`.
- `TrashRestore`: take selected id; `self.store.restore(id)`; on `Ok` reload the
  browser list + refresh trash list, status `"Restored"`; on
  `Err(Duplicate(_))` status `"Can't restore: conflicting equation exists"` and
  stay in trash.
- `TrashPurgeRequest`: `self.mode = Mode::ConfirmPurge(id)`.
- `ConfirmPurgeYes`: `self.store.purge(id)?; refresh trash list;` if trash empty,
  optionally drop back to Browser; status `"Permanently deleted"`.
- `ConfirmPurgeNo`: back to `Mode::Trash`.

### Rendering (`ui/mod.rs`, `ui/widgets.rs`)

- `ui/mod.rs`: add `Mode::Trash => widgets::trash(frame, app)` and render the
  `ConfirmPurge` confirmation like `ConfirmDelete`.
- `ui/widgets.rs`: add a `trash` widget — a bordered list of `trash_items`
  showing `name` + `deleted_at`, highlighting `trash_cursor`. Reuse the
  existing list styling. Empty state: "Trash is empty".
- Status-bar hints (`widgets.rs` ~line 311): add
  `Mode::Trash => "r restore  d delete  esc back"` and a `ConfirmPurge` hint.

## Tests

Core (`store/mod.rs` tests):

- `trash_then_list_trash_roundtrips` — insert, `trash`, assert gone from
  `all()`, present in `list_trash()` with right name.
- `restore_brings_back_equation_with_children` — full equation (vars/tags/refs),
  `trash`, `restore`, assert `get` returns equivalent content and trash empty.
- `restore_conflict_is_duplicate_error` — trash eq A (`E=mc^2`), insert new eq
  with same latex identity, `restore(A)` → `Err(Duplicate)`, A still in trash.
- `restore_drops_dangling_related_edges` — A related to B, trash A, delete B,
  restore A → succeeds, A has no related edges.
- `purge_removes_trash_row` — trash then `purge`, assert `list_trash` empty and
  `restore` now `NotFound`.
- `migration_v5_creates_trash_table` — open old-version DB, migrate, assert
  table exists and `user_version = 5`.

TUI (`app.rs` tests): extend the `command_matches` asserts for the new `trash`
command; add a small flow test if the existing harness supports driving
actions (open trash → restore/purge updates state).

## Out of scope / follow-ups

- Edit history (only delete/restore is covered here).
- Auto-expiry / size cap on trash.
- Resurrecting reverse `related` edges that pointed *at* a trashed equation.

## Note on current branch state

`app.rs` currently has in-progress cmdline compile errors flagged by
rust-analyzer (missing `OpenCmdline`/`CmdlineInput`/… match arm at the
`Action::None` site, and missing `width` struct fields around lines 2257-2297).
This plan builds on the cmdline scaffolding; those pre-existing errors should be
resolved (or will be, as part of the cmdline work) before/alongside landing the
trash command, since the trash command is dispatched through the same cmdline.
