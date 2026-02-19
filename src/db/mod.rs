mod migrations;

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

/// Open (or create) the SQLite database at `path` and run all pending migrations.
pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .with_context(|| format!("failed to open database at {}", path.display()))?;

    // WAL mode for better concurrency (pit dashboard + background reaper)
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;

    migrations::run(&conn)?;
    Ok(conn)
}

/// Open an in-memory database for testing. Runs all migrations.
#[cfg(test)]
pub fn open_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrations::run(&conn)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_memory_succeeds() {
        let conn = open_memory().unwrap();
        // Verify the tasks table exists
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn open_file_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("pit.db");
        let conn = open(&db_path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn open_file_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("pit.db");
        // Open twice — migrations should be idempotent
        let _conn1 = open(&db_path).unwrap();
        drop(_conn1);
        let conn2 = open(&db_path).unwrap();
        let count: i64 = conn2
            .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
