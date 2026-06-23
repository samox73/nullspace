mod migrations;

use crate::error::{Error, Result};
use crate::model::*;
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::Path;

pub struct Store {
    conn: Connection,
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
        let pattern = format!("%{}%", like_escape(&query.to_lowercase()));
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT e.id, e.name, e.description, e.latex, e.px_height
             FROM equations e
             LEFT JOIN tags t ON t.equation_id = e.id
             WHERE lower(e.name)        LIKE ?1 ESCAPE '\\'
                OR lower(e.description) LIKE ?1 ESCAPE '\\'
                OR lower(t.tag)         LIKE ?1 ESCAPE '\\'
             ORDER BY e.name COLLATE NOCASE",
        )?;
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

    pub fn import_equations(&mut self, equations: &[Equation]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for equation in equations {
            upsert_equation_row(&tx, equation)?;
            delete_children_conn(&tx, equation.id)?;
        }
        for equation in equations {
            insert_children(&tx, equation)?;
        }
        tx.commit()?;
        Ok(())
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
        insert_equation_row(&tx, eq)?;
        insert_children(&tx, eq)?;
        tx.commit()?;
        Ok(())
    }

    pub fn update(&mut self, eq: &Equation) -> Result<()> {
        let tx = self.conn.transaction()?;
        let changed = tx.execute(
            "UPDATE equations SET name=?2, description=?3, latex=?4, px_height=?5, created_at=?6, updated_at=?7 WHERE id=?1",
            params![
                eq.id.to_string(),
                eq.name,
                eq.description,
                eq.latex,
                eq.px_height as i64,
                eq.created_at,
                eq.updated_at
            ],
        )?;
        if changed == 0 {
            return Err(Error::NotFound(eq.id.to_string()));
        }
        delete_children(&tx, eq.id)?;
        insert_children(&tx, eq)?;
        tx.commit()?;
        Ok(())
    }

    pub fn delete(&mut self, id: EquationId) -> Result<()> {
        self.conn
            .execute("DELETE FROM equations WHERE id=?1", params![id.to_string()])?;
        Ok(())
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
        let mut stmt = self
            .conn
            .prepare("SELECT text, url FROM refs WHERE equation_id=?1 ORDER BY position")?;
        let rows = stmt.query_map(params![id], |row| {
            Ok(Reference {
                text: row.get(0)?,
                url: row.get(1)?,
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

fn insert_equation_row(conn: &Connection, eq: &Equation) -> Result<()> {
    conn.execute(
        "INSERT INTO equations (id, name, description, latex, px_height, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            eq.id.to_string(),
            eq.name,
            eq.description,
            eq.latex,
            eq.px_height as i64,
            eq.created_at,
            eq.updated_at
        ],
    )?;
    Ok(())
}

fn upsert_equation_row(conn: &Connection, eq: &Equation) -> Result<()> {
    conn.execute(
        "INSERT INTO equations (id, name, description, latex, px_height, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
            name=excluded.name,
            description=excluded.description,
            latex=excluded.latex,
            px_height=excluded.px_height,
            created_at=excluded.created_at,
            updated_at=excluded.updated_at",
        params![
            eq.id.to_string(),
            eq.name,
            eq.description,
            eq.latex,
            eq.px_height as i64,
            eq.created_at,
            eq.updated_at
        ],
    )?;
    Ok(())
}

fn insert_children(conn: &Connection, eq: &Equation) -> Result<()> {
    let id = eq.id.to_string();
    for (position, variable) in eq.variables.iter().enumerate() {
        conn.execute(
            "INSERT INTO variables (equation_id, symbol, description, position) VALUES (?1, ?2, ?3, ?4)",
            params![id, variable.symbol, variable.description, position as i64],
        )?;
    }
    for tag in &eq.tags {
        conn.execute(
            "INSERT INTO tags (equation_id, tag) VALUES (?1, ?2)",
            params![id, tag],
        )?;
    }
    for (position, reference) in eq.references.iter().enumerate() {
        conn.execute(
            "INSERT INTO refs (equation_id, text, url, position) VALUES (?1, ?2, ?3, ?4)",
            params![id, reference.text, reference.url, position as i64],
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
    Ok(EquationSummary {
        id: parse_id_col(&id)?,
        name: row.get(1)?,
        description: row.get(2)?,
        latex: row.get(3)?,
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

    fn full_equation(name: &str) -> Equation {
        let mut eq = Equation::new(name.to_string(), "E = mc^2".to_string());
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
            text: "Einstein".to_string(),
            url: Some("https://example.test".to_string()),
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
    fn search_matches_name_description_and_tags() {
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
        target.import_equations(&exported).unwrap();

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
            .insert(&Equation::new("500 off".to_string(), "x".to_string()))
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
}
