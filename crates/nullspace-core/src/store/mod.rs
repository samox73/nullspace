mod migrations;

use crate::error::{Error, Result};
use crate::identity::equation_identity;
use crate::model::*;
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub struct Store {
    conn: Connection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicatePolicy {
    Skip,
    Overwrite,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportSummary {
    pub inserted: usize,
    pub updated: usize,
    pub skipped: usize,
}

pub fn now_rfc3339() -> String {
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;
    OffsetDateTime::now_utc().format(&Rfc3339).unwrap()
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute("PRAGMA foreign_keys = ON", [])?;
        migrations::migrate(&conn)?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute("PRAGMA foreign_keys = ON", [])?;
        migrations::migrate(&conn)?;
        Ok(Self { conn })
    }

    pub fn list(&self) -> Result<Vec<EquationSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, latex, px_height FROM equations ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], summary_from_row)?;
        collect_summaries(rows)
    }

    pub fn search(&self, query: &str) -> Result<Vec<EquationSummary>> {
        let query = query.trim();
        if query.is_empty() {
            return self.list();
        }
        if let Some((scope, term)) = parse_search_scope(query) {
            return self.search_scoped(scope, term);
        }
        let pattern = format!("%{}%", like_escape(&query.to_lowercase()));
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT e.id, e.name, e.description, e.latex, e.px_height
             FROM equations e
             LEFT JOIN tags t ON t.equation_id = e.id
             WHERE lower(e.name)        LIKE ?1 ESCAPE '\\'
                OR lower(e.description) LIKE ?1 ESCAPE '\\'
                OR lower(e.latex)       LIKE ?1 ESCAPE '\\'
                OR lower(t.tag)         LIKE ?1 ESCAPE '\\'
             ORDER BY e.name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map(params![pattern], summary_from_row)?;
        collect_summaries(rows)
    }

    pub fn tag_counts(&self) -> Result<Vec<(String, usize)>> {
        let mut stmt = self.conn.prepare(
            "SELECT tag, COUNT(DISTINCT equation_id) AS item_count
             FROM tags
             GROUP BY tag
             ORDER BY item_count DESC, tag COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

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

    pub fn untagged_count(&self) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM equations e
             WHERE NOT EXISTS (SELECT 1 FROM tags t WHERE t.equation_id = e.id)",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    fn search_scoped(&self, scope: SearchScope, term: &str) -> Result<Vec<EquationSummary>> {
        let term = term.trim();
        if term.is_empty() {
            return self.list();
        }
        let pattern = format!("%{}%", like_escape(&term.to_lowercase()));
        let sql = match scope {
            SearchScope::Tag => {
                "SELECT DISTINCT e.id, e.name, e.description, e.latex, e.px_height
                 FROM equations e
                 JOIN tags t ON t.equation_id = e.id
                 WHERE lower(t.tag) LIKE ?1 ESCAPE '\\'
                 ORDER BY e.name COLLATE NOCASE"
            }
            SearchScope::Variable => {
                "SELECT DISTINCT e.id, e.name, e.description, e.latex, e.px_height
                 FROM equations e
                 JOIN variables v ON v.equation_id = e.id
                 WHERE lower(v.symbol) LIKE ?1 ESCAPE '\\'
                    OR lower(v.description) LIKE ?1 ESCAPE '\\'
                 ORDER BY e.name COLLATE NOCASE"
            }
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![pattern], summary_from_row)?;
        collect_summaries(rows)
    }

    pub fn by_symbol(&self, symbol: &str) -> Result<Vec<EquationSummary>> {
        let symbol = symbol.trim();
        if symbol.is_empty() {
            return self.list();
        }
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT e.id, e.name, e.description, e.latex, e.px_height
             FROM equations e
             JOIN variables v ON v.equation_id = e.id
             WHERE v.symbol = ?1
             ORDER BY e.name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map(params![symbol], summary_from_row)?;
        collect_summaries(rows)
    }

    pub fn all(&self) -> Result<Vec<Equation>> {
        self.list()?
            .into_iter()
            .map(|summary| self.get(summary.id))
            .collect()
    }

    pub fn import_equations(
        &mut self,
        equations: &[Equation],
        policy: DuplicatePolicy,
    ) -> Result<ImportSummary> {
        let tx = self.conn.transaction()?;
        let existing_ids = load_existing_ids(&tx)?;
        let mut existing_by_norm = load_existing_by_norm(&tx)?;
        let mut seen_norm = HashMap::new();
        let mut id_map = HashMap::new();
        let mut scheduled = Vec::new();
        let mut summary = ImportSummary::default();

        for equation in equations {
            let norm = equation_identity(&equation.latex);
            if existing_ids.contains(&equation.id) {
                seen_norm.insert(norm.clone(), equation.id);
                id_map.insert(equation.id, equation.id);
                scheduled.push(equation.clone());
                summary.updated += 1;
            } else if let Some(canonical_id) = seen_norm.get(&norm).copied() {
                id_map.insert(equation.id, canonical_id);
                summary.skipped += 1;
            } else if let Some(canonical_id) = existing_by_norm.get(&norm).copied() {
                id_map.insert(equation.id, canonical_id);
                seen_norm.insert(norm, canonical_id);
                match policy {
                    DuplicatePolicy::Skip => {
                        summary.skipped += 1;
                    }
                    DuplicatePolicy::Overwrite => {
                        let mut canonical = equation.clone();
                        canonical.id = canonical_id;
                        scheduled.push(canonical);
                        summary.updated += 1;
                    }
                }
            } else {
                seen_norm.insert(norm.clone(), equation.id);
                existing_by_norm.insert(norm, equation.id);
                id_map.insert(equation.id, equation.id);
                scheduled.push(equation.clone());
                summary.inserted += 1;
            }
        }

        for equation in &scheduled {
            upsert_equation_row(&tx, equation)?;
        }
        for equation in &scheduled {
            delete_children_conn(&tx, equation.id)?;
            let mut equation = equation.clone();
            equation.related = equation
                .related
                .into_iter()
                .map(|id| id_map.get(&id).copied().unwrap_or(id))
                .filter(|id| *id != equation.id)
                .collect();
            insert_children(&tx, &equation)?;
        }
        tx.commit()?;
        Ok(summary)
    }

    pub fn get(&self, id: EquationId) -> Result<Equation> {
        let id_s = id.to_string();
        let mut eq = self
            .conn
            .query_row(
                "SELECT id, name, description, latex, px_height, created_at, updated_at FROM equations WHERE id=?1",
                params![id_s],
                |row| {
                    let raw_id: String = row.get(0)?;
                    Ok(Equation {
                        id: parse_id_col(&raw_id)?,
                        name: row.get(1)?,
                        description: row.get(2)?,
                        latex: row.get(3)?,
                        px_height: row.get::<_, i64>(4)? as u32,
                        references: Vec::new(),
                        tags: Vec::new(),
                        variables: Vec::new(),
                        related: Vec::new(),
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(id.to_string()))?;

        eq.variables = self.load_variables(&id_s)?;
        eq.tags = self.load_tags(&id_s)?;
        eq.references = self.load_refs(&id_s)?;
        eq.related = self.load_related(&id_s)?;
        Ok(eq)
    }

    pub fn insert(&mut self, eq: &Equation) -> Result<()> {
        let tx = self.conn.transaction()?;
        insert_equation_row(&tx, eq, false)?;
        insert_children(&tx, eq)?;
        tx.commit()?;
        Ok(())
    }

    pub fn insert_allowing_duplicate_latex(&mut self, eq: &Equation) -> Result<()> {
        let tx = self.conn.transaction()?;
        insert_equation_row(&tx, eq, true)?;
        insert_children(&tx, eq)?;
        tx.commit()?;
        Ok(())
    }

    pub fn update(&mut self, eq: &Equation) -> Result<()> {
        let tx = self.conn.transaction()?;
        let latex_norm = equation_identity(&eq.latex);
        let changed = tx.execute(
            "UPDATE equations SET name=?2, description=?3, latex=?4, latex_norm=?5, px_height=?6, created_at=?7, updated_at=?8 WHERE id=?1",
            params![
                eq.id.to_string(),
                eq.name,
                eq.description,
                eq.latex,
                latex_norm,
                eq.px_height as i64,
                eq.created_at,
                eq.updated_at
            ],
        ).map_err(|err| duplicate_or_db(err, &eq.latex))?;
        if changed == 0 {
            return Err(Error::NotFound(eq.id.to_string()));
        }
        delete_children(&tx, eq.id)?;
        insert_children(&tx, eq)?;
        tx.commit()?;
        Ok(())
    }

    pub fn update_px_height(&self, id: EquationId, px_height: u32) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE equations SET px_height=?2 WHERE id=?1",
            params![id.to_string(), px_height as i64],
        )?;
        if changed == 0 {
            return Err(Error::NotFound(id.to_string()));
        }
        Ok(())
    }

    pub fn delete(&mut self, id: EquationId) -> Result<()> {
        self.conn
            .execute("DELETE FROM equations WHERE id=?1", params![id.to_string()])?;
        Ok(())
    }

    pub fn trash(&mut self, id: EquationId) -> Result<()> {
        let eq = self.get(id)?;
        let snapshot = serde_json::to_string(&eq)?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO trash (id, name, snapshot, deleted_at) VALUES (?1, ?2, ?3, ?4)",
            params![id.to_string(), eq.name, snapshot, now_rfc3339()],
        )?;
        tx.execute("DELETE FROM equations WHERE id=?1", params![id.to_string()])?;
        tx.commit()?;
        Ok(())
    }

    pub fn restore(&mut self, id: EquationId) -> Result<()> {
        let tx = self.conn.transaction()?;
        let id_s = id.to_string();
        let snapshot = tx
            .query_row(
                "SELECT snapshot FROM trash WHERE id=?1",
                params![id_s],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        let mut eq: Equation = serde_json::from_str(&snapshot)?;
        insert_equation_row(&tx, &eq, false)?;
        // Related edges pointing to equations deleted after this row was trashed
        // cannot be restored because the target row no longer exists.
        eq.related = existing_equation_ids(&tx, &eq.related)?;
        insert_children(&tx, &eq)?;
        tx.execute("DELETE FROM trash WHERE id=?1", params![id.to_string()])?;
        tx.commit()?;
        Ok(())
    }

    pub fn purge(&mut self, id: EquationId) -> Result<()> {
        let changed = self
            .conn
            .execute("DELETE FROM trash WHERE id=?1", params![id.to_string()])?;
        if changed == 0 {
            return Err(Error::NotFound(id.to_string()));
        }
        Ok(())
    }

    pub fn list_trash(&self) -> Result<Vec<TrashEntry>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, deleted_at FROM trash ORDER BY deleted_at DESC")?;
        let rows = stmt.query_map([], |row| {
            let raw_id: String = row.get(0)?;
            Ok(TrashEntry {
                id: parse_id_col(&raw_id)?,
                name: row.get(1)?,
                deleted_at: row.get(2)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn child_count_for_tests(&self, id: EquationId) -> Result<i64> {
        let id_s = id.to_string();
        let vars: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM variables WHERE equation_id=?1",
            params![id_s],
            |row| row.get(0),
        )?;
        let tags: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM tags WHERE equation_id=?1",
            params![id_s],
            |row| row.get(0),
        )?;
        let refs: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM refs WHERE equation_id=?1",
            params![id_s],
            |row| row.get(0),
        )?;
        let rel: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM related WHERE a=?1 OR b=?1",
            params![id_s],
            |row| row.get(0),
        )?;
        Ok(vars + tags + refs + rel)
    }

    fn load_variables(&self, id: &str) -> Result<Vec<Variable>> {
        let mut stmt = self.conn.prepare(
            "SELECT symbol, description FROM variables WHERE equation_id=?1 ORDER BY position",
        )?;
        let rows = stmt.query_map(params![id], |row| {
            Ok(Variable {
                symbol: row.get(0)?,
                description: row.get(1)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    fn load_tags(&self, id: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT tag FROM tags WHERE equation_id=?1 ORDER BY tag COLLATE NOCASE")?;
        let rows = stmt.query_map(params![id], |row| row.get(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    fn load_refs(&self, id: &str) -> Result<Vec<Reference>> {
        let mut stmt = self.conn.prepare(
            "SELECT authors, year, title, doi, url, pages FROM refs WHERE equation_id=?1 ORDER BY position",
        )?;
        let rows = stmt.query_map(params![id], |row| {
            Ok(Reference {
                authors: row.get(0)?,
                year: row.get(1)?,
                title: row.get(2)?,
                doi: row.get(3)?,
                url: row.get(4)?,
                pages: row.get(5)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    fn load_related(&self, id: &str) -> Result<Vec<EquationId>> {
        let mut stmt = self
            .conn
            .prepare("SELECT a, b FROM related WHERE a=?1 OR b=?1 ORDER BY a, b")?;
        let rows = stmt.query_map(params![id], |row| {
            let a: String = row.get(0)?;
            let b: String = row.get(1)?;
            let other = if a == id { b } else { a };
            parse_id_col(&other)
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchScope {
    Tag,
    Variable,
}

fn parse_search_scope(query: &str) -> Option<(SearchScope, &str)> {
    let (scope, term) = query.split_once(':')?;
    let scope = match scope.trim().to_ascii_lowercase().as_str() {
        "tag" => SearchScope::Tag,
        "var" => SearchScope::Variable,
        _ => return None,
    };
    Some((scope, term))
}

fn insert_equation_row(
    conn: &Connection,
    eq: &Equation,
    allow_duplicate_latex: bool,
) -> Result<()> {
    let latex_norm = equation_identity(&eq.latex);
    conn.execute(
        "INSERT INTO equations (id, name, description, latex, latex_norm, px_height, created_at, updated_at, allow_duplicate_latex) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            eq.id.to_string(),
            eq.name,
            eq.description,
            eq.latex,
            latex_norm,
            eq.px_height as i64,
            eq.created_at,
            eq.updated_at,
            allow_duplicate_latex as i64,
        ],
    )
    .map_err(|err| duplicate_or_db(err, &eq.latex))?;
    Ok(())
}

fn upsert_equation_row(conn: &Connection, eq: &Equation) -> Result<()> {
    let latex_norm = equation_identity(&eq.latex);
    conn.execute(
        "INSERT INTO equations (id, name, description, latex, latex_norm, px_height, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET
            name=excluded.name,
            description=excluded.description,
            latex=excluded.latex,
            latex_norm=excluded.latex_norm,
            px_height=excluded.px_height,
            created_at=excluded.created_at,
            updated_at=excluded.updated_at",
        params![
            eq.id.to_string(),
            eq.name,
            eq.description,
            eq.latex,
            latex_norm,
            eq.px_height as i64,
            eq.created_at,
            eq.updated_at
        ],
    )
    .map_err(|err| duplicate_or_db(err, &eq.latex))?;
    Ok(())
}

fn load_existing_ids(conn: &Connection) -> Result<HashSet<EquationId>> {
    let mut stmt = conn.prepare("SELECT id FROM equations")?;
    let rows = stmt.query_map([], |row| {
        let raw_id: String = row.get(0)?;
        parse_id_col(&raw_id)
    })?;
    rows.collect::<std::result::Result<HashSet<_>, _>>()
        .map_err(Into::into)
}

fn load_existing_by_norm(conn: &Connection) -> Result<HashMap<String, EquationId>> {
    let mut stmt = conn.prepare("SELECT id, latex FROM equations")?;
    let rows = stmt.query_map([], |row| {
        let raw_id: String = row.get(0)?;
        let latex: String = row.get(1)?;
        Ok((equation_identity(&latex), parse_id_col(&raw_id)?))
    })?;
    rows.collect::<std::result::Result<HashMap<_, _>, _>>()
        .map_err(Into::into)
}

fn existing_equation_ids(conn: &Connection, ids: &[EquationId]) -> Result<Vec<EquationId>> {
    let mut existing = Vec::new();
    for id in ids {
        let found = conn
            .query_row(
                "SELECT 1 FROM equations WHERE id=?1",
                params![id.to_string()],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if found {
            existing.push(*id);
        }
    }
    Ok(existing)
}

fn duplicate_or_db(err: rusqlite::Error, latex: &str) -> Error {
    if is_unique_violation(&err) {
        Error::Duplicate(latex.to_string())
    } else {
        Error::Db(err)
    }
}

fn is_unique_violation(err: &rusqlite::Error) -> bool {
    matches!(err, rusqlite::Error::SqliteFailure(e, _)
        if e.code == rusqlite::ErrorCode::ConstraintViolation
            && e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE)
}

fn insert_children(conn: &Connection, eq: &Equation) -> Result<()> {
    let id = eq.id.to_string();
    for (position, variable) in eq.variables.iter().enumerate() {
        conn.execute(
            "INSERT OR IGNORE INTO variables (equation_id, symbol, description, position) VALUES (?1, ?2, ?3, ?4)",
            params![id, variable.symbol, variable.description, position as i64],
        )?;
    }
    for tag in &eq.tags {
        conn.execute(
            "INSERT OR IGNORE INTO tags (equation_id, tag) VALUES (?1, ?2)",
            params![id, tag],
        )?;
    }
    for (position, reference) in eq.references.iter().enumerate() {
        conn.execute(
            "INSERT INTO refs (equation_id, authors, year, title, doi, url, pages, position)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                reference.authors,
                reference.year,
                reference.title,
                reference.doi,
                reference.url,
                reference.pages,
                position as i64
            ],
        )?;
    }
    for related in &eq.related {
        let other = related.to_string();
        if other == id {
            continue;
        }
        let (a, b) = if id < other {
            (id.clone(), other)
        } else {
            (other, id.clone())
        };
        conn.execute(
            "INSERT OR IGNORE INTO related (a, b) VALUES (?1, ?2)",
            params![a, b],
        )?;
    }
    Ok(())
}

fn delete_children(conn: &Connection, id: EquationId) -> Result<()> {
    delete_children_conn(conn, id)
}

fn delete_children_conn(conn: &Connection, id: EquationId) -> Result<()> {
    let id = id.to_string();
    conn.execute("DELETE FROM variables WHERE equation_id=?1", params![id])?;
    conn.execute("DELETE FROM tags WHERE equation_id=?1", params![id])?;
    conn.execute("DELETE FROM refs WHERE equation_id=?1", params![id])?;
    conn.execute("DELETE FROM related WHERE a=?1 OR b=?1", params![id])?;
    Ok(())
}

fn summary_from_row(row: &Row<'_>) -> rusqlite::Result<EquationSummary> {
    let id: String = row.get(0)?;
    let latex: String = row.get(3)?;
    let unicode_approx = crate::render::to_unicode_approx(&latex);
    Ok(EquationSummary {
        id: parse_id_col(&id)?,
        name: row.get(1)?,
        description: row.get(2)?,
        latex,
        unicode_approx,
        px_height: row.get::<_, i64>(4)? as u32,
    })
}

fn like_escape(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn parse_id_col(raw: &str) -> rusqlite::Result<EquationId> {
    EquationId::parse(raw).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(crate::error::Error::NotFound(format!(
                "invalid uuid: {raw}"
            ))),
        )
    })
}

fn collect_summaries<I>(rows: I) -> Result<Vec<EquationSummary>>
where
    I: Iterator<Item = rusqlite::Result<EquationSummary>>,
{
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn full_equation(name: &str) -> Equation {
        let mut eq = Equation::new(name.to_string(), format!("{name} = mc^2"));
        eq.description = "mass energy".to_string();
        eq.px_height = 48;
        eq.variables = vec![
            Variable {
                symbol: "E".to_string(),
                description: "energy".to_string(),
            },
            Variable {
                symbol: "m".to_string(),
                description: "mass".to_string(),
            },
        ];
        eq.tags = vec!["physics".to_string(), "relativity".to_string()];
        eq.references = vec![Reference {
            authors: "Einstein".to_string(),
            year: Some(1905),
            title: "Annalen der Physik".to_string(),
            doi: None,
            url: Some("https://example.test".to_string()),
            pages: Some("1-4".to_string()),
        }];
        eq
    }

    #[test]
    fn insert_then_get_roundtrip() {
        let mut store = Store::open_in_memory().unwrap();
        let eq = full_equation("Energy");
        store.insert(&eq).unwrap();
        let got = store.get(eq.id).unwrap();
        assert_eq!(got.name, eq.name);
        assert_eq!(got.description, eq.description);
        assert_eq!(got.latex, eq.latex);
        assert_eq!(got.variables, eq.variables);
        assert_eq!(got.tags, eq.tags);
        assert_eq!(got.references, eq.references);
    }

    #[test]
    fn insert_allowing_duplicate_latex_preserves_formula() {
        let mut store = Store::open_in_memory().unwrap();
        let original = Equation::new("Original".to_string(), "E=mc^2".to_string());
        store.insert(&original).unwrap();

        let duplicate = Equation::new("Clone".to_string(), original.latex.clone());
        let err = store.insert(&duplicate).unwrap_err();
        assert!(matches!(err, Error::Duplicate(_)));

        store.insert_allowing_duplicate_latex(&duplicate).unwrap();
        let got = store.get(duplicate.id).unwrap();
        assert_eq!(got.latex, original.latex);
        assert_eq!(store.all().unwrap().len(), 2);
    }

    #[test]
    fn update_px_height_only_changes_px_height() {
        let mut store = Store::open_in_memory().unwrap();
        let eq = full_equation("Energy");
        let id = eq.id;
        store.insert(&eq).unwrap();
        let before = store.get(id).unwrap();
        let child_count = store.child_count_for_tests(id).unwrap();

        store.update_px_height(id, 96).unwrap();

        let after = store.get(id).unwrap();
        assert_eq!(after.px_height, 96);
        assert_eq!(after.name, before.name);
        assert_eq!(after.description, before.description);
        assert_eq!(after.latex, before.latex);
        assert_eq!(after.variables, before.variables);
        assert_eq!(after.tags, before.tags);
        assert_eq!(after.references, before.references);
        assert_eq!(after.updated_at, before.updated_at);
        assert_eq!(store.child_count_for_tests(id).unwrap(), child_count);
    }

    #[test]
    fn list_returns_summaries_sorted_by_name() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .insert(&Equation::new("Zeta".to_string(), "z".to_string()))
            .unwrap();
        store
            .insert(&Equation::new("Alpha".to_string(), "a".to_string()))
            .unwrap();
        let names: Vec<_> = store.list().unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["Alpha", "Zeta"]);
    }

    #[test]
    fn list_summaries_include_unicode_approximation() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .insert(&Equation::new(
                "Energy".to_string(),
                "\\alpha^2".to_string(),
            ))
            .unwrap();

        let summary = store.list().unwrap().pop().unwrap();

        assert_eq!(
            summary.unicode_approx,
            crate::render::to_unicode_approx("\\alpha^2")
        );
    }

    #[test]
    fn search_matches_name_description_latex_and_tags() {
        let mut store = Store::open_in_memory().unwrap();
        let mut energy = full_equation("Energy");
        energy.description = "mass energy".to_string();
        energy.tags = vec!["physics".to_string()];
        let mut area = Equation::new("Area".to_string(), "A = \\pi r^2".to_string());
        area.description = "circle".to_string();
        area.tags = vec!["geometry".to_string()];
        store.insert(&energy).unwrap();
        store.insert(&area).unwrap();

        let physics = store.search("physics").unwrap();
        assert_eq!(physics.len(), 1);
        assert_eq!(physics[0].name, "Energy");
        let circle = store.search("circle").unwrap();
        assert_eq!(circle.len(), 1);
        assert_eq!(circle[0].name, "Area");
        let latex = store.search("\\pi").unwrap();
        assert_eq!(latex.len(), 1);
        assert_eq!(latex[0].name, "Area");
    }

    #[test]
    fn search_supports_tag_and_variable_prefixes() {
        let mut store = Store::open_in_memory().unwrap();
        let mut energy = full_equation("Energy");
        energy.tags = vec!["physics".to_string()];
        let mut area = Equation::new("Area".to_string(), "A = \\pi r^2".to_string());
        area.description = "circle".to_string();
        area.tags = vec!["geometry".to_string()];
        area.variables = vec![Variable {
            symbol: "A".to_string(),
            description: "area".to_string(),
        }];
        store.insert(&energy).unwrap();
        store.insert(&area).unwrap();
        energy.related.push(area.id);
        store.update(&energy).unwrap();

        let tag = store.search("tag:phys").unwrap();
        assert_eq!(tag.len(), 1);
        assert_eq!(tag[0].name, "Energy");
        let variable = store.search("var:area").unwrap();
        assert_eq!(variable.len(), 1);
        assert_eq!(variable[0].name, "Area");
    }

    #[test]
    fn removed_search_prefixes_are_not_scoped() {
        let mut store = Store::open_in_memory().unwrap();
        let area = Equation::new("Area".to_string(), "A = \\pi r^2".to_string());
        store.insert(&area).unwrap();

        assert!(store.search("name:area").unwrap().is_empty());
        assert!(store.search("latex:\\pi").unwrap().is_empty());
        assert!(store.search("related:area").unwrap().is_empty());
    }

    #[test]
    fn tag_counts_are_sorted_by_frequency() {
        let mut store = Store::open_in_memory().unwrap();
        let mut first = full_equation("First");
        first.tags = vec!["diagmc".to_string(), "dft".to_string()];
        let mut second = full_equation("Second");
        second.tags = vec!["diagmc".to_string()];
        let mut third = full_equation("Third");
        third.tags = vec!["polaron".to_string()];
        store.insert(&first).unwrap();
        store.insert(&second).unwrap();
        store.insert(&third).unwrap();

        let counts = store.tag_counts().unwrap();

        assert_eq!(
            counts,
            vec![
                ("diagmc".to_string(), 2),
                ("dft".to_string(), 1),
                ("polaron".to_string(), 1),
            ]
        );
    }

    #[test]
    fn by_tag_matches_exactly() {
        let mut store = Store::open_in_memory().unwrap();
        let mut dft = full_equation("DFT");
        dft.tags = vec!["dft".to_string()];
        let mut dft_plus_u = full_equation("DFT+U");
        dft_plus_u.tags = vec!["dft-plus-u".to_string()];
        store.insert(&dft).unwrap();
        store.insert(&dft_plus_u).unwrap();

        let matches = store.by_tag("dft").unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "DFT");
    }

    #[test]
    fn by_tag_is_case_insensitive() {
        let mut store = Store::open_in_memory().unwrap();
        let mut eq = full_equation("Energy");
        eq.tags = vec!["DFT".to_string()];
        store.insert(&eq).unwrap();

        let matches = store.by_tag("dft").unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "Energy");
    }

    #[test]
    fn untagged_returns_only_equations_without_tags() {
        let mut store = Store::open_in_memory().unwrap();
        let tagged = full_equation("Tagged");
        let untagged = Equation::new("Untagged".to_string(), "x".to_string());
        store.insert(&tagged).unwrap();
        store.insert(&untagged).unwrap();

        let matches = store.untagged().unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "Untagged");
    }

    #[test]
    fn untagged_count_matches_untagged_len() {
        let mut store = Store::open_in_memory().unwrap();
        let tagged = full_equation("Tagged");
        let first = Equation::new("First untagged".to_string(), "a".to_string());
        let second = Equation::new("Second untagged".to_string(), "b".to_string());
        store.insert(&tagged).unwrap();
        store.insert(&first).unwrap();
        store.insert(&second).unwrap();

        assert_eq!(
            store.untagged_count().unwrap(),
            store.untagged().unwrap().len()
        );
    }

    #[test]
    fn search_unknown_prefix_falls_back_to_general_search() {
        let mut store = Store::open_in_memory().unwrap();
        let eq = Equation::new("Energy".to_string(), "source:external".to_string());
        store.insert(&eq).unwrap();

        let results = store.search("source:external").unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Energy");
    }

    #[test]
    fn by_symbol_returns_equations_using_variable() {
        let mut store = Store::open_in_memory().unwrap();
        let energy = full_equation("Energy");
        let mut area = Equation::new("Area".to_string(), "A = \\pi r^2".to_string());
        area.variables = vec![Variable {
            symbol: "A".to_string(),
            description: "area".to_string(),
        }];
        store.insert(&energy).unwrap();
        store.insert(&area).unwrap();

        let matches = store.by_symbol("E").unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "Energy");
    }

    #[test]
    fn all_and_import_equations_roundtrip_related_records() {
        let mut source = Store::open_in_memory().unwrap();
        let mut a = full_equation("A");
        let b = full_equation("B");
        source.insert(&a).unwrap();
        source.insert(&b).unwrap();
        a.related.push(b.id);
        source.update(&a).unwrap();

        let exported = source.all().unwrap();
        let mut target = Store::open_in_memory().unwrap();
        target
            .import_equations(&exported, DuplicatePolicy::Skip)
            .unwrap();

        assert_eq!(target.all().unwrap().len(), 2);
        assert!(target.get(a.id).unwrap().related.contains(&b.id));
        assert!(target.get(b.id).unwrap().related.contains(&a.id));
    }

    #[test]
    fn update_replaces_children() {
        let mut store = Store::open_in_memory().unwrap();
        let mut eq = full_equation("Energy");
        store.insert(&eq).unwrap();
        eq.variables = vec![Variable {
            symbol: "c".to_string(),
            description: "speed of light".to_string(),
        }];
        eq.tags = vec!["updated".to_string()];
        eq.references.clear();
        store.update(&eq).unwrap();
        let got = store.get(eq.id).unwrap();
        assert_eq!(got.variables, eq.variables);
        assert_eq!(got.tags, vec!["updated"]);
        assert!(got.references.is_empty());
    }

    #[test]
    fn delete_cascades() {
        let mut store = Store::open_in_memory().unwrap();
        let eq = full_equation("Energy");
        let id = eq.id;
        store.insert(&eq).unwrap();
        store.delete(id).unwrap();
        assert_eq!(store.child_count_for_tests(id).unwrap(), 0);
    }

    #[test]
    fn trash_then_list_trash_roundtrips() {
        let mut store = Store::open_in_memory().unwrap();
        let eq = full_equation("Energy");
        let id = eq.id;
        store.insert(&eq).unwrap();

        store.trash(id).unwrap();

        assert!(store.all().unwrap().is_empty());
        let trash = store.list_trash().unwrap();
        assert_eq!(trash.len(), 1);
        assert_eq!(trash[0].id, id);
        assert_eq!(trash[0].name, "Energy");
        assert!(!trash[0].deleted_at.is_empty());
    }

    #[test]
    fn restore_brings_back_equation_with_children() {
        let mut store = Store::open_in_memory().unwrap();
        let eq = full_equation("Energy");
        let id = eq.id;
        store.insert(&eq).unwrap();

        store.trash(id).unwrap();
        store.restore(id).unwrap();

        let got = store.get(id).unwrap();
        assert_eq!(got.name, eq.name);
        assert_eq!(got.description, eq.description);
        assert_eq!(got.latex, eq.latex);
        assert_eq!(got.variables, eq.variables);
        assert_eq!(got.tags, eq.tags);
        assert_eq!(got.references, eq.references);
        assert!(store.list_trash().unwrap().is_empty());
    }

    #[test]
    fn restore_conflict_is_duplicate_error() {
        let mut store = Store::open_in_memory().unwrap();
        let eq = Equation::new("Energy".to_string(), "E=mc^2".to_string());
        let id = eq.id;
        store.insert(&eq).unwrap();
        store.trash(id).unwrap();
        store
            .insert(&Equation::new(
                "Replacement".to_string(),
                "E = mc^2".to_string(),
            ))
            .unwrap();

        let err = store.restore(id).unwrap_err();

        assert!(matches!(err, Error::Duplicate(_)));
        assert_eq!(store.list_trash().unwrap().len(), 1);
    }

    #[test]
    fn restore_drops_dangling_related_edges() {
        let mut store = Store::open_in_memory().unwrap();
        let mut a = Equation::new("A".to_string(), "a".to_string());
        let b = Equation::new("B".to_string(), "b".to_string());
        let a_id = a.id;
        let b_id = b.id;
        store.insert(&a).unwrap();
        store.insert(&b).unwrap();
        a.related.push(b_id);
        store.update(&a).unwrap();

        store.trash(a_id).unwrap();
        store.delete(b_id).unwrap();
        store.restore(a_id).unwrap();

        assert!(store.get(a_id).unwrap().related.is_empty());
    }

    #[test]
    fn purge_removes_trash_row() {
        let mut store = Store::open_in_memory().unwrap();
        let eq = full_equation("Energy");
        let id = eq.id;
        store.insert(&eq).unwrap();
        store.trash(id).unwrap();

        store.purge(id).unwrap();

        assert!(store.list_trash().unwrap().is_empty());
        assert!(matches!(store.restore(id), Err(Error::NotFound(_))));
    }

    #[test]
    fn related_is_bidirectional() {
        let mut store = Store::open_in_memory().unwrap();
        let mut a = Equation::new("A".to_string(), "a".to_string());
        let b = Equation::new("B".to_string(), "b".to_string());
        store.insert(&a).unwrap();
        store.insert(&b).unwrap();
        a.related.push(b.id);
        store.update(&a).unwrap();
        assert!(store.get(a.id).unwrap().related.contains(&b.id));
        assert!(store.get(b.id).unwrap().related.contains(&a.id));
    }

    #[test]
    fn get_missing_is_not_found() {
        let store = Store::open_in_memory().unwrap();
        let id = EquationId::new();
        assert!(matches!(store.get(id), Err(Error::NotFound(s)) if s == id.to_string()));
    }

    #[test]
    fn search_percent_is_literal_not_wildcard() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .insert(&Equation::new("50% off".to_string(), "x".to_string()))
            .unwrap();
        store
            .insert(&Equation::new("500 off".to_string(), "y".to_string()))
            .unwrap();
        let results = store.search("50%").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "50% off");
    }

    #[test]
    fn search_is_case_insensitive() {
        let mut store = Store::open_in_memory().unwrap();
        let mut eq = Equation::new("Energy".to_string(), "E=mc^2".to_string());
        eq.tags = vec!["physics".to_string()];
        store.insert(&eq).unwrap();
        let results = store.search("PHYSICS").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Energy");
    }

    #[test]
    fn insert_rejects_duplicate_latex_identity() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .insert(&Equation::new("Energy".to_string(), "E=mc^2".to_string()))
            .unwrap();
        let err = store
            .insert(&Equation::new(
                "Energy 2".to_string(),
                "E = mc^2".to_string(),
            ))
            .unwrap_err();
        assert!(matches!(err, Error::Duplicate(latex) if latex == "E = mc^2"));
    }

    #[test]
    fn insert_allows_distinct_latex_identity() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .insert(&Equation::new("Upper".to_string(), "\\Pi".to_string()))
            .unwrap();
        store
            .insert(&Equation::new("Lower".to_string(), "\\pi".to_string()))
            .unwrap();
        assert_eq!(store.all().unwrap().len(), 2);
    }

    #[test]
    fn import_skips_content_duplicate() {
        let mut store = Store::open_in_memory().unwrap();
        let first = Equation::new("First".to_string(), "E=mc^2".to_string());
        store
            .import_equations(&[first], DuplicatePolicy::Skip)
            .unwrap();
        let second = Equation::new("Second".to_string(), "E = mc^2".to_string());

        let summary = store
            .import_equations(&[second], DuplicatePolicy::Skip)
            .unwrap();

        assert_eq!(
            summary,
            ImportSummary {
                inserted: 0,
                updated: 0,
                skipped: 1
            }
        );
        assert_eq!(store.all().unwrap().len(), 1);
    }

    #[test]
    fn import_same_file_twice_is_idempotent() {
        let mut store = Store::open_in_memory().unwrap();
        let equations = vec![
            Equation::new("A".to_string(), "a".to_string()),
            Equation::new("B".to_string(), "b".to_string()),
        ];

        let first = store
            .import_equations(&equations, DuplicatePolicy::Skip)
            .unwrap();
        let second = store
            .import_equations(&equations, DuplicatePolicy::Skip)
            .unwrap();

        assert_eq!(
            first,
            ImportSummary {
                inserted: 2,
                updated: 0,
                skipped: 0
            }
        );
        assert_eq!(
            second,
            ImportSummary {
                inserted: 0,
                updated: 2,
                skipped: 0
            }
        );
        assert_eq!(store.all().unwrap().len(), 2);
    }

    #[test]
    fn import_overwrite_updates_canonical() {
        let mut store = Store::open_in_memory().unwrap();
        let canonical = Equation::new("Canonical".to_string(), "E=mc^2".to_string());
        let canonical_id = canonical.id;
        store
            .import_equations(&[canonical], DuplicatePolicy::Skip)
            .unwrap();
        let mut incoming = Equation::new("Replacement".to_string(), "E = mc^2".to_string());
        incoming.description = "new metadata".to_string();

        let summary = store
            .import_equations(&[incoming], DuplicatePolicy::Overwrite)
            .unwrap();
        let got = store.get(canonical_id).unwrap();

        assert_eq!(
            summary,
            ImportSummary {
                inserted: 0,
                updated: 1,
                skipped: 0
            }
        );
        assert_eq!(store.all().unwrap().len(), 1);
        assert_eq!(got.name, "Replacement");
        assert_eq!(got.description, "new metadata");
    }

    #[test]
    fn import_remaps_related_to_canonical() {
        let mut store = Store::open_in_memory().unwrap();
        let canonical = Equation::new("Canonical".to_string(), "E=mc^2".to_string());
        let canonical_id = canonical.id;
        store
            .import_equations(&[canonical], DuplicatePolicy::Skip)
            .unwrap();

        let duplicate = Equation::new("Duplicate".to_string(), "E = mc^2".to_string());
        let mut new_eq = Equation::new("New".to_string(), "x".to_string());
        new_eq.related.push(duplicate.id);
        let new_id = new_eq.id;

        let summary = store
            .import_equations(&[duplicate, new_eq], DuplicatePolicy::Skip)
            .unwrap();

        assert_eq!(
            summary,
            ImportSummary {
                inserted: 1,
                updated: 0,
                skipped: 1
            }
        );
        assert!(store.get(new_id).unwrap().related.contains(&canonical_id));
        assert!(store.get(canonical_id).unwrap().related.contains(&new_id));
    }

    #[test]
    fn migration_dedups_existing_rows_and_preserves_relations() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE equations (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                latex       TEXT NOT NULL,
                px_height   INTEGER NOT NULL DEFAULT 48,
                created_at  TEXT NOT NULL,
                updated_at  TEXT NOT NULL
            );
            CREATE TABLE related (
                a TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
                b TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
                PRIMARY KEY (a, b),
                CHECK (a < b)
            );
            PRAGMA user_version = 1;
            "#,
        )
        .unwrap();
        let survivor = EquationId::new().to_string();
        let duplicate = EquationId::new().to_string();
        let other = EquationId::new().to_string();
        conn.execute(
            "INSERT INTO equations (id, name, description, latex, px_height, created_at, updated_at)
             VALUES (?1, 'survivor', '', 'E=mc^2', 48, '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
            params![survivor],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO equations (id, name, description, latex, px_height, created_at, updated_at)
             VALUES (?1, 'duplicate', '', 'E = mc^2', 48, '2024-01-02T00:00:00Z', '2024-01-02T00:00:00Z')",
            params![duplicate],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO equations (id, name, description, latex, px_height, created_at, updated_at)
             VALUES (?1, 'other', '', 'x', 48, '2024-01-03T00:00:00Z', '2024-01-03T00:00:00Z')",
            params![other],
        )
        .unwrap();
        let (a, b) = if duplicate < other {
            (duplicate.clone(), other.clone())
        } else {
            (other.clone(), duplicate.clone())
        };
        conn.execute("INSERT INTO related (a, b) VALUES (?1, ?2)", params![a, b])
            .unwrap();

        migrations::migrate(&conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM equations", [], |row| row.get(0))
            .unwrap();
        let duplicate_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM equations WHERE id=?1",
                params![duplicate],
                |row| row.get(0),
            )
            .unwrap();
        let relation_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM related WHERE (a=?1 AND b=?2) OR (a=?2 AND b=?1)",
                params![survivor, other],
                |row| row.get(0),
            )
            .unwrap();
        let index_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_equations_latex_norm'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(count, 2);
        assert_eq!(duplicate_count, 0);
        assert_eq!(relation_count, 1);
        assert_eq!(index_count, 1);
    }

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

        let (title, url, authors, pages): (String, Option<String>, String, Option<String>) = conn
            .query_row(
                "SELECT title, url, authors, pages FROM refs WHERE equation_id=?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(title, "Kohn & Sham 1965");
        assert_eq!(
            url.as_deref(),
            Some("https://doi.org/10.1103/PhysRev.140.A1133")
        );
        assert_eq!(authors, "");
        assert_eq!(pages, None);
    }

    #[test]
    fn migration_v5_creates_trash_table() {
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
            CREATE TABLE variables (
                equation_id TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
                symbol TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
                position INTEGER NOT NULL,
                PRIMARY KEY (equation_id, symbol)
            );
            CREATE TABLE tags (
                equation_id TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
                tag TEXT NOT NULL,
                PRIMARY KEY (equation_id, tag)
            );
            CREATE TABLE refs (
                equation_id TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
                authors TEXT NOT NULL DEFAULT '', year INTEGER,
                title TEXT NOT NULL DEFAULT '', doi TEXT, url TEXT,
                position INTEGER NOT NULL
            );
            CREATE TABLE related (
                a TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
                b TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
                PRIMARY KEY (a, b),
                CHECK (a < b)
            );
            PRAGMA user_version = 4;
            "#,
        )
        .unwrap();

        migrations::migrate(&conn).unwrap();

        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='trash'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let user_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(table_count, 1);
        assert_eq!(user_version, 6);
    }

    #[test]
    fn migration_v6_adds_ref_pages() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE refs (
                equation_id TEXT NOT NULL,
                authors TEXT NOT NULL DEFAULT '', year INTEGER,
                title TEXT NOT NULL DEFAULT '', doi TEXT, url TEXT,
                position INTEGER NOT NULL
            );
            PRAGMA user_version = 5;
            "#,
        )
        .unwrap();

        migrations::migrate(&conn).unwrap();

        let pages_exists: bool = conn
            .prepare("PRAGMA table_info(refs)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
            .iter()
            .any(|column| column == "pages");
        let user_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert!(pages_exists);
        assert_eq!(user_version, 6);
    }
}
