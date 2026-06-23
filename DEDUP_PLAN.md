# Nullspace — equation de-duplication plan

Goal: make normalized LaTeX the identity of an equation, enforced in the database, so the
library can never hold two entries with the same formula — whether they arrive via import
or are typed in the editor. Existing duplicates are merged by a migration.

## Decisions (locked)

- **Identity = parsed-AST canonical form.** Two LaTeX strings are the same equation iff
  they parse (via ratex) to the same abstract syntax tree. This correctly treats
  `E=mc^2` ≡ `E = mc^2` (insignificant spaces dropped), while keeping `\alpha beta` ≠
  `\alphabeta` (control-word termination), `a\ b` (escaped space), and `\text{a b}` ≠
  `\text{ab}` (text-mode literal spaces) distinct — none of which a whitespace rule can do.
  No lowercasing (the parser is case-sensitive: `\Pi` ≠ `\pi`, `E` ≠ `e`).
- **Enforced with a `UNIQUE` index** on a stored `latex_norm` column (the canonical key
  string; see §1 for why string, not hash).
- **Import default policy = `skip`.** Flag: `--on-duplicate=skip|overwrite` (see note).
- **A migration de-duplicates existing rows** before the unique index is created.
- **Import prints a summary** (`inserted / updated / skipped`).

> Consequence of the UNIQUE index: a "keep both" import option is impossible by
> construction (the DB rejects it), so the only policies are **skip** and **overwrite**.
> Re-importing the *same* export stays idempotent because rows are matched by `id` first.

---

## 1. `equation_identity` (core)

New `pub fn equation_identity(latex: &str) -> String` in `nullspace-core` (e.g.
`src/identity.rs`, re-exported from `lib.rs`). Used by store insert/update, the migration,
and import — one definition, everywhere. **No new dependencies:** core already pulls in
`ratex-parser`, `serde`, and `serde_json`.

```rust
const IDENTITY_VERSION: u32 = 1; // bump when ratex is upgraded or the rule changes

pub fn equation_identity(latex: &str) -> String {
    match ratex_parser::parser::parse(latex) {
        Ok(nodes) => {
            // Serialize the AST, then canonicalize:
            //   1. strip every `loc` (source offsets) so spacing/position can't leak in
            //   2. serde_json::Value uses a sorted BTreeMap -> deterministic key order
            //      (also fixes the `attributes: HashMap` node's ordering)
            let mut value = serde_json::to_value(&nodes).unwrap_or(serde_json::Value::Null);
            strip_keys(&mut value, "loc");
            format!("ast:v{IDENTITY_VERSION}:{}", serde_json::to_string(&value).unwrap_or_default())
        }
        // Unparseable latex (can't render anyway): fall back to whitespace-collapse,
        // tagged so it can never collide with an AST key.
        Err(_) => format!("raw:v{IDENTITY_VERSION}:{}",
            latex.split_whitespace().collect::<Vec<_>>().join(" ")),
    }
}

fn strip_keys(value: &mut serde_json::Value, key: &str) {
    match value {
        serde_json::Value::Object(map) => {
            map.remove(key);
            for v in map.values_mut() { strip_keys(v, key); }
        }
        serde_json::Value::Array(items) => {
            for v in items.iter_mut() { strip_keys(v, key); }
        }
        _ => {}
    }
}
```

**Why store the string, not a hash.** The canonical string is deterministic and stable,
so it works directly as the UNIQUE key with zero hash-stability concerns and no extra
dependency. Equations are small/few, so key size is a non-issue. (If size ever matters,
hashing the canonical string with a *stable* algorithm — e.g. blake3 — is a drop-in
optimization; do **not** use `DefaultHasher`, which isn't stable across Rust versions.)

**Determinism prerequisites (verify, they hold today):**
- `serde_json`'s `preserve_order` feature must stay **off** (checked: it is) so `Value`
  sorts object keys. If any future dep turns it on, switch to an explicitly key-sorted
  serialization.
- `loc` stripping makes the key independent of source positions; confirmed `ParseNode`'s
  only position data is the `loc` field on each variant.

**Versioning.** `IDENTITY_VERSION` (and the prefix) is baked into the key. When ratex is
upgraded or the rule changes, bump it and add a migration step that recomputes `latex_norm`
for all rows and re-runs dedup (same machinery as §3).

**Tests:**
- `equation_identity("E=mc^2") == equation_identity("E = mc^2")` (spaces ignored).
- `equation_identity("\\alpha beta") != equation_identity("\\alphabeta")` (control word).
- `equation_identity("\\text{a b}") != equation_identity("\\text{ab}")` (text-mode space).
- `equation_identity("\\Pi") != equation_identity("\\pi")` (case-sensitive).
- Calling it twice on the same input yields identical output (determinism guard).
- Unparseable input (e.g. `"\\frac{"`) yields a `raw:`-prefixed key and doesn't panic.

---

## 2. Schema + migration (`store/migrations.rs`)

Switch to versioned migrations keyed on `PRAGMA user_version` (currently `0`).

### 2.1 Migration runner
```rust
pub fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 { migrate_v1(conn)?; }   // existing CREATE TABLE IF NOT EXISTS batch
    if version < 2 { migrate_v2(conn)?; }   // dedup + latex_norm + unique index
    Ok(())
}
```
- `migrate_v1` = the current `execute_batch(SCHEMA)` (unchanged). End it with
  `PRAGMA user_version = 1` for brand-new DBs.

### 2.2 Migration v2 — order matters
Add column → backfill → **dedup** → create unique index → bump version. The index must be
created *after* dedup, or it fails on existing duplicates.

```rust
fn migrate_v2(conn: &Connection) -> Result<()> {
    // 1. add column (guard against re-run)
    if !column_exists(conn, "equations", "latex_norm")? {
        conn.execute("ALTER TABLE equations ADD COLUMN latex_norm TEXT NOT NULL DEFAULT ''", [])?;
    }
    // 2. backfill in Rust (SQLite core can't collapse whitespace)
    backfill_latex_norm(conn)?;
    // 3. merge existing duplicates (see §3)
    dedup_existing(conn)?;
    // 4. enforce uniqueness from now on
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_equations_latex_norm ON equations(latex_norm)",
        [],
    )?;
    conn.execute("PRAGMA user_version = 2", [])?;
    Ok(())
}
```
- `column_exists` via `PRAGMA table_info(equations)`.
- `backfill_latex_norm`: `SELECT id, latex`, compute `equation_identity`, `UPDATE equations
  SET latex_norm=?2 WHERE id=?1`. Run inside the migration's transaction.

Run the whole of `migrate` in a single transaction so a failure leaves the DB untouched.

---

## 3. De-dup algorithm for existing rows (`dedup_existing`)

Group existing rows by `latex_norm`; for each group with >1 row pick a **survivor** and
fold the others into it, **remapping relations** so no link dangles.

```
SELECT latex_norm, id, created_at FROM equations ORDER BY latex_norm, created_at, id
for each group sharing latex_norm with size > 1:
    survivor = first row (min created_at, tie-break min id)
    for each dup in the rest:
        // remap related links to the survivor
        for other_id in related-partners-of(dup)        // SELECT a,b WHERE a=dup OR b=dup
            if other_id != survivor:
                (lo, hi) = sort(survivor, other_id)
                INSERT OR IGNORE INTO related(a,b) VALUES (lo, hi)   // skip self & dup pairs
        DELETE FROM related WHERE a=dup OR b=dup
        DELETE FROM equations WHERE id=dup        // cascade clears dup's children
```

**Metadata policy (survivor wins).** The survivor keeps its own name / description / tags /
variables / references; the duplicates' metadata is dropped. Relations are the only thing
merged (so cross-references survive). This is predictable and simple.
- *Optional enhancement (note, don't build now):* union the dups' tags into the survivor.
  Skipped to keep the migration deterministic and reviewable.

**Edge cases handled:** survivor↔dup mutual relation → would form a self-pair, skipped;
duplicate (lo,hi) pairs → `INSERT OR IGNORE`; deleting the dup row cascades its
variables/tags/refs and any leftover `related` rows still pointing at it.

---

## 4. Store write paths (`store/mod.rs`)

Every write must populate `latex_norm`, and UNIQUE violations must become a typed error,
not a crash.

### 4.1 Populate `latex_norm`
- `insert_equation_row` and `upsert_equation_row`: add `latex_norm` to the column list and
  params, value = `equation_identity(&eq.latex)`. Include it in `ON CONFLICT(id) DO UPDATE`.
- `update`: add `latex_norm=?N` to the `SET` clause.

### 4.2 Typed duplicate error
- New variant in `error.rs`: `#[error("duplicate equation: {0}")] Duplicate(String)`.
- Helper to classify rusqlite errors:
  ```rust
  fn is_unique_violation(err: &rusqlite::Error) -> bool {
      matches!(err, rusqlite::Error::SqliteFailure(e, _)
          if e.code == rusqlite::ErrorCode::ConstraintViolation
          && e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE)
  }
  ```
- `insert`/`update`: on a unique violation, return
  `Error::Duplicate(eq.latex.clone())` instead of the raw `Error::Db`.

---

## 5. Editor save handling (`app.rs`)

The UNIQUE index now affects in-app creates/edits, so `persist_editor` must degrade
gracefully (currently it would surface a raw DB error in the status line / propagate).
- When `store.insert`/`store.update` returns `Error::Duplicate(_)`, set a friendly status:
  `"An equation with this LaTeX already exists."` and **do not** treat it as fatal — keep
  the editor open with the user's text. Clear the dirty flag bookkeeping so autosave
  doesn't hammer the same failing write every 300 ms (e.g. remember the last failed
  signature and skip until the latex changes).

---

## 6. Import rewrite (`store/mod.rs::import_equations`)

New signature:
```rust
pub enum DuplicatePolicy { Skip, Overwrite }   // default Skip

pub struct ImportSummary { pub inserted: usize, pub updated: usize, pub skipped: usize }

pub fn import_equations(
    &mut self,
    equations: &[Equation],
    policy: DuplicatePolicy,
) -> Result<ImportSummary>
```

Algorithm, all in one transaction:

1. **Load existing state:** `existing_ids: HashSet<String>` and
   `existing_by_norm: HashMap<String, EquationId>` (from `SELECT id, latex`).
2. **First pass — classify** each incoming equation and build
   `id_map: HashMap<EquationId, EquationId>` (incoming id → surviving canonical id) plus a
   within-batch `seen_norm: HashMap<String, EquationId>`:
   - `norm = equation_identity(&eq.latex)`
   - **same id exists** → it's the same record: schedule an **update** under `eq.id`;
     `id_map[eq.id] = eq.id`; `updated += 1`.
   - else **norm already seen in this batch** → `id_map[eq.id] = seen_norm[norm]`;
     `skipped += 1`.
   - else **norm exists in DB (different id)**:
     - `Skip` → `id_map[eq.id] = existing_by_norm[norm]`; `skipped += 1`; write nothing.
     - `Overwrite` → schedule an **overwrite of the canonical row** (keep canonical id,
       take incoming content); `id_map[eq.id] = canonical`; `updated += 1`.
   - else **brand new** → schedule **insert** under `eq.id`; `seen_norm[norm] = eq.id`;
     `existing_by_norm.insert(norm, eq.id)`; `inserted += 1`.
3. **Write equation rows** (loop 1) for every scheduled insert/update/overwrite, with
   `latex_norm`. Doing all rows before any children guarantees FK targets exist.
4. **Write children** (loop 2) for scheduled rows only: delete-then-insert children, and
   translate each `related` id through `id_map` (drop self-pairs, `INSERT OR IGNORE`).
   Skipped equations contribute nothing (their canonical row is left untouched).
5. `commit`, return the `ImportSummary`.

Notes:
- Re-importing the same export → every row hits "same id exists" → all `updated`, none
  duplicated (idempotent, matches current behavior).
- Because uniqueness is enforced, step 2 cannot produce a constraint violation; but still
  wrap the write so an unexpected one maps to `Error::Duplicate`.

---

## 7. CLI (`main.rs`)

- Parse `--on-duplicate <skip|overwrite>` (default `Skip`); unknown value → error.
- `import_json` calls `store.import_equations(&equations, policy)` and prints the summary:
  ```
  imported {inserted} new, updated {updated}, skipped {skipped} duplicate(s) from {path}
  ```
- Update `--help` text to mention `--on-duplicate`.

---

## 8. Tests

Core (`store/mod.rs` `#[cfg(test)]`):
1. `equation_identity_*` (see §1: spacing, control word, text-mode, case, determinism,
   unparseable fallback).
2. `insert_rejects_duplicate_latex` — inserting two equations whose latex normalizes equal
   returns `Error::Duplicate` on the second.
3. `insert_allows_spacing_variants` — `E=mc^2` and `E = mc^2` both insert (distinct norms).
4. `import_skips_content_duplicate` — import `E=mc^2` (id A); import again with id B same
   latex, policy `Skip` → `ImportSummary { inserted:0, updated:0, skipped:1 }`, store has 1.
5. `import_same_file_twice_is_idempotent` — same ids both times → all `updated`, count
   stable.
6. `import_overwrite_updates_canonical` — policy `Overwrite` replaces canonical content,
   keeps the canonical id, store still has 1.
7. `import_remaps_related_to_canonical` — batch with eq X and a skipped dup of an existing
   eq Y, where X.related = [dup-id]; after import X relates to the canonical Y id.
8. `migration_dedups_existing_rows` — build a v1-style DB (no `latex_norm`) with two rows
   sharing latex + a relation into one of them; run `migrate`; expect one survivor, the
   relation preserved on the survivor, and the unique index present.

CLI smoke (manual or script): repeat the §"verify" import twice + content-dup runs from
the review; assert counts via `--export` + a JSON length check.

---

## 9. Acceptance gate
```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
Plus manual: open an existing DB (triggers v2 migration), confirm it opens, dups are
merged, and creating a duplicate equation in the editor shows the friendly message.

---

## 10. Risks / notes

- **Migration is destructive (merges rows).** It runs inside a transaction so a failure
  rolls back, but back up the DB file before first run on real data. Survivor-wins means
  duplicate rows' *metadata* is discarded (relations are preserved).
- **Identity is tied to the ratex parser.** A ratex upgrade can change the AST / its
  serialization and thus every key. Guard with `IDENTITY_VERSION` (§1): bump it on a ratex
  change and add a migration step that recomputes `latex_norm` and re-dedups. Pin ratex in
  `Cargo.toml` so upgrades are deliberate.
- **Determinism depends on `serde_json` not enabling `preserve_order`** and on `loc` being
  the only positional field — both verified today; add the determinism test (§1) so a
  regression is caught in CI rather than as silent duplicate rows.
- **Unparseable latex** uses the `raw:` whitespace fallback, so two invalid strings dedupe
  only if textually equal after whitespace-collapse. Acceptable (such entries can't render
  anyway), but it means identity is weaker for non-rendering latex.
- **`latex_norm` must stay in sync** with `latex` on every write — centralize by always
  computing it from `eq.latex` inside the store write helpers, never from callers.
- **Order dependency** in v2 (backfill → dedup → unique index) is load-bearing; keep it.
