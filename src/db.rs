use std::path::Path;

use rusqlite::{params, Connection};

/// A git project registered in the local database.
#[derive(Debug, Clone)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub added_at: String,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    path TEXT NOT NULL UNIQUE,
    added_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE IF NOT EXISTS credentials (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    label TEXT NOT NULL,
    nonce BLOB NOT NULL,
    ciphertext BLOB NOT NULL,
    UNIQUE(project_id, label)
);
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value BLOB NOT NULL
);
"#;

/// Open (creating if needed) the database at `path` and ensure the schema exists.
pub fn init_db(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

/// Insert a project and return its row id. Fails if `path` already exists.
pub fn add_project(conn: &Connection, name: &str, path: &str) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO projects (name, path) VALUES (?1, ?2)",
        params![name, path],
    )?;
    Ok(conn.last_insert_rowid())
}

/// List all projects, oldest first.
pub fn list_projects(conn: &Connection) -> rusqlite::Result<Vec<Project>> {
    let mut stmt =
        conn.prepare("SELECT id, name, path, added_at FROM projects ORDER BY id ASC")?;
    let rows = stmt.query_map([], |row| {
        Ok(Project {
            id: row.get(0)?,
            name: row.get(1)?,
            path: row.get(2)?,
            added_at: row.get(3)?,
        })
    })?;
    rows.collect()
}

/// Delete the project row only; the working directory on disk is never touched.
pub fn remove_project(conn: &Connection, id: i64) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM projects WHERE id = ?1", params![id])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn add_and_list_roundtrip() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let repo_path = repo.to_str().unwrap();

        let id = add_project(&conn, "demo", repo_path).unwrap();
        assert!(id > 0);

        let projects = list_projects(&conn).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, id);
        assert_eq!(projects[0].name, "demo");
        assert_eq!(projects[0].path, repo_path);
        assert!(!projects[0].added_at.is_empty());
    }

    #[test]
    fn duplicate_path_rejected() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let repo_path = repo.to_str().unwrap();

        add_project(&conn, "demo", repo_path).unwrap();
        assert!(add_project(&conn, "other", repo_path).is_err());
    }

    #[test]
    fn remove_keeps_directory() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let probe = repo.join(".keep");
        fs::write(&probe, b"probe").unwrap();
        let repo_path = repo.to_str().unwrap();

        let id = add_project(&conn, "demo", repo_path).unwrap();
        let removed = remove_project(&conn, id).unwrap();
        assert_eq!(removed, 1);
        assert!(list_projects(&conn).unwrap().is_empty());
        assert!(
            probe.exists(),
            "remove_project must not touch the filesystem"
        );
        assert!(repo.exists());
    }
}
