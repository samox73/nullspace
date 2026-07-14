use crate::error::Result;
use crate::identity::equation_identity;
use rusqlite::{Connection, OptionalExtension, params};

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS equations (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    latex       TEXT NOT NULL,
    px_height   INTEGER NOT NULL DEFAULT 48,
    assumptions TEXT NOT NULL DEFAULT '',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS quantities (
    id          TEXT PRIMARY KEY,
    symbol      TEXT NOT NULL,
    name        TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    units       TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS variables (
    equation_id TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
    symbol      TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    position    INTEGER NOT NULL,
    quantity_id TEXT REFERENCES quantities(id) ON DELETE SET NULL,
    PRIMARY KEY (equation_id, symbol)
);

CREATE TABLE IF NOT EXISTS tags (
    equation_id TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
    tag         TEXT NOT NULL,
    PRIMARY KEY (equation_id, tag)
);
CREATE INDEX IF NOT EXISTS idx_tags_tag ON tags(tag);

CREATE TABLE IF NOT EXISTS refs (
    equation_id TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
    authors     TEXT NOT NULL DEFAULT '',
    year        INTEGER,
    title       TEXT NOT NULL DEFAULT '',
    doi         TEXT,
    url         TEXT,
    pages       TEXT,
    position    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS related (
    a TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
    b TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
    PRIMARY KEY (a, b),
    CHECK (a < b)
);

CREATE TABLE IF NOT EXISTS trash (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    snapshot    TEXT NOT NULL,
    deleted_at  TEXT NOT NULL
);
"#;

pub fn migrate(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version < 1 {
        migrate_v1(&tx)?;
    }
    if version < 2 {
        migrate_v2(&tx)?;
    }
    if version < 3 {
        migrate_v3(&tx)?;
    }
    if version < 4 {
        migrate_v4(&tx)?;
    }
    if version < 5 {
        migrate_v5(&tx)?;
    }
    if version < 6 {
        migrate_v6(&tx)?;
    }
    if version < 7 {
        migrate_v7(&tx)?;
    }
    tx.commit()?;
    Ok(())
}

fn migrate_v1(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    // For databases created before px_height was added; new databases already have
    // the column from SCHEMA above, so ignore the error if it already exists.
    let _ = conn.execute(
        "ALTER TABLE equations ADD COLUMN px_height INTEGER NOT NULL DEFAULT 48",
        [],
    );
    conn.pragma_update(None, "user_version", 1_i64)?;
    Ok(())
}

fn migrate_v2(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "equations", "latex_norm")? {
        conn.execute(
            "ALTER TABLE equations ADD COLUMN latex_norm TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    backfill_latex_norm(conn)?;
    dedup_existing(conn)?;
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_equations_latex_norm ON equations(latex_norm)",
        [],
    )?;
    conn.pragma_update(None, "user_version", 2_i64)?;
    Ok(())
}

fn migrate_v3(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "equations", "allow_duplicate_latex")? {
        conn.execute(
            "ALTER TABLE equations ADD COLUMN allow_duplicate_latex INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    conn.execute("DROP INDEX IF EXISTS idx_equations_latex_norm", [])?;
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_equations_latex_norm
         ON equations(latex_norm)
         WHERE allow_duplicate_latex = 0",
        [],
    )?;
    conn.pragma_update(None, "user_version", 3_i64)?;
    Ok(())
}

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
                pages       TEXT,
                position    INTEGER NOT NULL
            );
            INSERT INTO refs_new (equation_id, authors, year, title, doi, url, pages, position)
                SELECT equation_id, '', NULL, text, NULL, url, NULL, position FROM refs;
            DROP TABLE refs;
            ALTER TABLE refs_new RENAME TO refs;
            "#,
        )?;
    }
    conn.pragma_update(None, "user_version", 4_i64)?;
    Ok(())
}

fn migrate_v5(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS trash (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            snapshot    TEXT NOT NULL,
            deleted_at  TEXT NOT NULL
        )",
        [],
    )?;
    conn.pragma_update(None, "user_version", 5_i64)?;
    Ok(())
}

fn migrate_v6(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "refs")? {
        conn.execute(
            "CREATE TABLE refs (
                equation_id TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
                authors     TEXT NOT NULL DEFAULT '',
                year        INTEGER,
                title       TEXT NOT NULL DEFAULT '',
                doi         TEXT,
                url         TEXT,
                pages       TEXT,
                position    INTEGER NOT NULL
            )",
            [],
        )?;
    }
    if !column_exists(conn, "refs", "pages")? {
        conn.execute("ALTER TABLE refs ADD COLUMN pages TEXT", [])?;
    }
    conn.pragma_update(None, "user_version", 6_i64)?;
    Ok(())
}

fn migrate_v7(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS quantities (
            id          TEXT PRIMARY KEY,
            symbol      TEXT NOT NULL,
            name        TEXT NOT NULL DEFAULT '',
            description TEXT NOT NULL DEFAULT '',
            units       TEXT NOT NULL DEFAULT ''
        )",
        [],
    )?;
    if table_exists(conn, "equations")? && !column_exists(conn, "equations", "assumptions")? {
        conn.execute(
            "ALTER TABLE equations ADD COLUMN assumptions TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    if table_exists(conn, "variables")? && !column_exists(conn, "variables", "quantity_id")? {
        conn.execute(
            "ALTER TABLE variables ADD COLUMN quantity_id TEXT REFERENCES quantities(id) ON DELETE SET NULL",
            [],
        )?;
    }
    conn.pragma_update(None, "user_version", 7_i64)?;
    Ok(())
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let found = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            params![table],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    Ok(found)
}

fn backfill_latex_norm(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("SELECT id, latex FROM equations")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let equations = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);

    for (id, latex) in equations {
        let norm = equation_identity(&latex);
        conn.execute(
            "UPDATE equations SET latex_norm=?2 WHERE id=?1",
            params![id, norm],
        )?;
    }
    Ok(())
}

fn dedup_existing(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT latex_norm, id, created_at FROM equations ORDER BY latex_norm, created_at, id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let rows = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);

    let mut index = 0;
    while index < rows.len() {
        let norm = &rows[index].0;
        let group_start = index;
        while index < rows.len() && rows[index].0 == *norm {
            index += 1;
        }
        if index - group_start <= 1 {
            continue;
        }

        let survivor = rows[group_start].1.clone();
        for (_, duplicate, _) in &rows[group_start + 1..index] {
            merge_duplicate(conn, &survivor, duplicate)?;
        }
    }
    Ok(())
}

fn merge_duplicate(conn: &Connection, survivor: &str, duplicate: &str) -> Result<()> {
    let mut stmt = conn.prepare("SELECT a, b FROM related WHERE a=?1 OR b=?1")?;
    let rows = stmt.query_map(params![duplicate], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let related = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);

    for (a, b) in related {
        let other = if a == duplicate { b } else { a };
        if other == survivor {
            continue;
        }
        let (lo, hi) = if survivor < other.as_str() {
            (survivor.to_string(), other)
        } else {
            (other, survivor.to_string())
        };
        conn.execute(
            "INSERT OR IGNORE INTO related (a, b) VALUES (?1, ?2)",
            params![lo, hi],
        )?;
    }
    conn.execute("DELETE FROM related WHERE a=?1 OR b=?1", params![duplicate])?;
    conn.execute("DELETE FROM equations WHERE id=?1", params![duplicate])?;
    Ok(())
}
