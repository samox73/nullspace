use crate::error::Result;
use rusqlite::Connection;

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS equations (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    latex       TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS variables (
    equation_id TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
    symbol      TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    position    INTEGER NOT NULL,
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
    text        TEXT NOT NULL,
    url         TEXT,
    position    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS related (
    a TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
    b TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
    PRIMARY KEY (a, b),
    CHECK (a < b)
);
"#;

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    Ok(())
}
