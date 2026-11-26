use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::crypto::{self, CryptoError, VaultKeys};

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
    let mut stmt = conn.prepare("SELECT id, name, path, added_at FROM projects ORDER BY id ASC")?;
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

/// Encrypt `secret` and upsert it as the credential `label` of `project_id`.
///
/// Only the nonce and ciphertext are persisted; the plaintext secret is never
/// written to the database nor included in error messages.
pub fn store_credential(
    conn: &Connection,
    keys: &VaultKeys,
    project_id: i64,
    label: &str,
    secret: &str,
) -> Result<(), CryptoError> {
    let (nonce, ciphertext) = crypto::encrypt(keys, secret.as_bytes())?;
    conn.execute(
        "INSERT OR REPLACE INTO credentials (project_id, label, nonce, ciphertext)
         VALUES (?1, ?2, ?3, ?4)",
        params![project_id, label, nonce, ciphertext],
    )
    .map_err(|e| CryptoError::Database(e.to_string()))?;
    Ok(())
}

/// Decrypt and return the credential `label` of `project_id`, or `None` when the
/// project has no such credential.
pub fn get_credential(
    conn: &Connection,
    keys: &VaultKeys,
    project_id: i64,
    label: &str,
) -> Result<Option<String>, CryptoError> {
    let stored = conn
        .query_row(
            "SELECT nonce, ciphertext FROM credentials WHERE project_id = ?1 AND label = ?2",
            params![project_id, label],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()
        .map_err(|e| CryptoError::Database(e.to_string()))?;

    let Some((nonce, ciphertext)) = stored else {
        return Ok(None);
    };
    let plaintext = crypto::decrypt(keys, &nonce, &ciphertext)?;
    String::from_utf8(plaintext)
        .map(Some)
        .map_err(|_| CryptoError::Database("stored credential is not valid UTF-8".into()))
}

/// Delete the credential `label` of `project_id`, returning the number of rows
/// removed (0 when it did not exist).
pub fn delete_credential(
    conn: &Connection,
    project_id: i64,
    label: &str,
) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM credentials WHERE project_id = ?1 AND label = ?2",
        params![project_id, label],
    )
}

/// Read the raw blob stored under `key` in the `meta` table, or `None` when
/// the key is absent.
pub fn get_meta(conn: &Connection, key: &str) -> rusqlite::Result<Option<Vec<u8>>> {
    conn.query_row(
        "SELECT value FROM meta WHERE key = ?1",
        params![key],
        |row| row.get::<_, Vec<u8>>(0),
    )
    .optional()
}

/// Insert or replace the raw blob stored under `key` in the `meta` table.
pub fn set_meta(conn: &Connection, key: &str, value: &[u8]) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{self, CryptoError, VaultKeys};
    use std::fs;
    use tempfile::tempdir;
    use tempfile::TempDir;

    const MASTER: &str = "correct horse battery staple";

    /// Fresh vault with one stored project. Returns the connection, the unlocked
    /// keys and the project id.
    fn vault_with_project(dir: &TempDir) -> (Connection, VaultKeys, i64) {
        let conn = init_db(&dir.path().join("creds.db")).unwrap();
        crypto::setup_master(&conn, MASTER).unwrap();
        let keys = crypto::unlock(&conn, MASTER).unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let id = add_project(&conn, "demo", repo.to_str().unwrap()).unwrap();
        (conn, keys, id)
    }

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

    #[test]
    fn credential_roundtrip() {
        let dir = tempdir().unwrap();
        let (conn, keys, project_id) = vault_with_project(&dir);

        store_credential(&conn, &keys, project_id, "git_token", "s3cret!").unwrap();
        let restored = get_credential(&conn, &keys, project_id, "git_token").unwrap();
        assert_eq!(restored.as_deref(), Some("s3cret!"));

        assert_eq!(
            delete_credential(&conn, project_id, "git_token").unwrap(),
            1
        );
        assert!(get_credential(&conn, &keys, project_id, "git_token")
            .unwrap()
            .is_none());
        assert_eq!(
            delete_credential(&conn, project_id, "git_token").unwrap(),
            0,
            "deleting a missing credential removes no rows"
        );
    }

    #[test]
    fn credential_missing_is_none() {
        let dir = tempdir().unwrap();
        let (conn, keys, project_id) = vault_with_project(&dir);

        let missing = get_credential(&conn, &keys, project_id, "never_stored").unwrap();
        assert!(missing.is_none());

        // A different project must not see the first project's credential.
        store_credential(&conn, &keys, project_id, "git_token", "s3cret!").unwrap();
        let repo = dir.path().join("other");
        fs::create_dir(&repo).unwrap();
        let other_id = add_project(&conn, "other", repo.to_str().unwrap()).unwrap();
        assert!(get_credential(&conn, &keys, other_id, "git_token")
            .unwrap()
            .is_none());
    }

    #[test]
    fn wrong_key_credential_fails() {
        let dir = tempdir().unwrap();
        let (conn, keys, project_id) = vault_with_project(&dir);
        store_credential(&conn, &keys, project_id, "git_token", "s3cret!").unwrap();

        let salt: Vec<u8> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params!["kdf_salt"],
                |row| row.get(0),
            )
            .unwrap();
        let wrong_keys = crypto::derive_keys("wrong master password", &salt);

        let err = match get_credential(&conn, &wrong_keys, project_id, "git_token") {
            Ok(_) => panic!("wrong keys must not decrypt a credential"),
            Err(e) => e,
        };
        assert!(matches!(err, CryptoError::WrongMaster));
    }

    #[test]
    fn delete_project_cascades_credentials() {
        let dir = tempdir().unwrap();
        let (conn, keys, project_id) = vault_with_project(&dir);
        store_credential(&conn, &keys, project_id, "git_token", "s3cret!").unwrap();

        remove_project(&conn, project_id).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM credentials", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 0,
            "ON DELETE CASCADE must remove credentials with their project"
        );
    }

    #[test]
    fn meta_get_set_roundtrip() {
        let dir = tempdir().unwrap();
        let conn = init_db(&dir.path().join("meta.db")).unwrap();

        assert!(
            get_meta(&conn, "no_such_key").unwrap().is_none(),
            "absent meta key must read as None"
        );

        set_meta(&conn, "probe", b"first").unwrap();
        assert_eq!(get_meta(&conn, "probe").unwrap().as_deref(), Some(b"first".as_slice()));

        // set_meta upserts: a second write replaces the previous value.
        set_meta(&conn, "probe", b"second").unwrap();
        assert_eq!(get_meta(&conn, "probe").unwrap().as_deref(), Some(b"second".as_slice()));

        // Binary values (non-UTF-8) survive the roundtrip.
        let blob: Vec<u8> = (0u8..=255).collect();
        set_meta(&conn, "blob", &blob).unwrap();
        assert_eq!(get_meta(&conn, "blob").unwrap().as_deref(), Some(blob.as_slice()));
    }
}
