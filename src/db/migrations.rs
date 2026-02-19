use anyhow::Result;
use rusqlite::Connection;

/// Each migration is a (version, description, sql) tuple.
/// Migrations are applied in order. The `schema_version` table tracks which have run.
const MIGRATIONS: &[(i64, &str, &str)] = &[
    (1, "initial schema", MIGRATION_001),
    (2, "add prompt and issue_url", MIGRATION_002),
    (3, "add agent column", MIGRATION_003),
];

const MIGRATION_001: &str = "
CREATE TABLE tasks (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL UNIQUE,
    description TEXT    NOT NULL DEFAULT '',
    branch      TEXT    NOT NULL,
    worktree    TEXT    NOT NULL,
    status      TEXT    NOT NULL DEFAULT 'idle'
                        CHECK (status IN ('idle', 'running', 'done', 'error')),
    session_id  TEXT,
    tmux_session TEXT,
    pid         INTEGER,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);
";

const MIGRATION_002: &str = "
ALTER TABLE tasks ADD COLUMN prompt TEXT NOT NULL DEFAULT '';
ALTER TABLE tasks ADD COLUMN issue_url TEXT NOT NULL DEFAULT '';
";

const MIGRATION_003: &str = "
ALTER TABLE tasks ADD COLUMN agent TEXT NOT NULL DEFAULT 'claude';
";

/// Run all pending migrations inside a transaction.
pub fn run(conn: &Connection) -> Result<()> {
    // Ensure the schema_version table exists
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version     INTEGER PRIMARY KEY,
            description TEXT NOT NULL,
            applied_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;

    let current_version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    for &(version, description, sql) in MIGRATIONS {
        if version > current_version {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_version (version, description) VALUES (?1, ?2)",
                rusqlite::params![version, description],
            )?;
            tx.commit()?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        run(&conn).unwrap(); // second call should be a no-op

        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 3);
    }

    #[test]
    fn tasks_table_has_expected_columns() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();

        // Insert a row using all columns to verify schema
        conn.execute(
            "INSERT INTO tasks (name, branch, worktree, status)
             VALUES ('test', 'pit/test', '/tmp/wt', 'idle')",
            [],
        )
        .unwrap();

        let (name, branch, status): (String, String, String) = conn
            .query_row(
                "SELECT name, branch, status FROM tasks WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();

        assert_eq!(name, "test");
        assert_eq!(branch, "pit/test");
        assert_eq!(status, "idle");
    }

    #[test]
    fn status_check_constraint_rejects_invalid() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();

        let result = conn.execute(
            "INSERT INTO tasks (name, branch, worktree, status)
             VALUES ('bad', 'pit/bad', '/tmp/wt', 'invalid')",
            [],
        );
        assert!(result.is_err());
    }
}
