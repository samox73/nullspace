mod migrations;

use crate::error::{Error, Result};
use crate::model::*;
use rusqlite::{params, Connection, OptionalExtension};
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
            "SELECT id, name, description, latex FROM equations ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            Ok(EquationSummary {
                id: EquationId::parse(&id).expect("stored ids are valid UUIDs"),
                name: row.get(1)?,
                description: row.get(2)?,
                latex: row.get(3)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get(&self, id: EquationId) -> Result<Equation> {
        let id_s = id.to_string();
        let mut eq = self
            .conn
            .query_row(
                "SELECT id, name, description, latex, created_at, updated_at FROM equations WHERE id=?1",
                params![id_s],
                |row| {
                    let raw_id: String = row.get(0)?;
                    Ok(Equation {
                        id: EquationId::parse(&raw_id).expect("stored ids are valid UUIDs"),
                        name: row.get(1)?,
                        description: row.get(2)?,
                        latex: row.get(3)?,
                        references: Vec::new(),
                        tags: Vec::new(),
                        variables: Vec::new(),
                        related: Vec::new(),
                        created_at: row.get(4)?,
                        updated_at: row.get(5)?,
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
            "UPDATE equations SET name=?2, description=?3, latex=?4, created_at=?5, updated_at=?6 WHERE id=?1",
            params![
                eq.id.to_string(),
                eq.name,
                eq.description,
                eq.latex,
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
            Ok(EquationId::parse(&other).expect("stored ids are valid UUIDs"))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}

fn insert_equation_row(conn: &Connection, eq: &Equation) -> Result<()> {
    conn.execute(
        "INSERT INTO equations (id, name, description, latex, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            eq.id.to_string(),
            eq.name,
            eq.description,
            eq.latex,
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
    let id = id.to_string();
    conn.execute("DELETE FROM variables WHERE equation_id=?1", params![id])?;
    conn.execute("DELETE FROM tags WHERE equation_id=?1", params![id])?;
    conn.execute("DELETE FROM refs WHERE equation_id=?1", params![id])?;
    conn.execute("DELETE FROM related WHERE a=?1 OR b=?1", params![id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_equation(name: &str) -> Equation {
        let mut eq = Equation::new(name.to_string(), "E = mc^2".to_string());
        eq.description = "mass energy".to_string();
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
}
