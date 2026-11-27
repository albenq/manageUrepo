use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::crypto::{self, CryptoError, VaultKeys};

/// A git or svn project registered in the local database.
#[derive(Debug, Clone)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub added_at: String,
    /// Which version control system this project uses: `"git"` or `"svn"`.
    pub vcs: String,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    path TEXT NOT NULL UNIQUE,
    added_at TEXT NOT NULL DEFAULT (datetime('now')),
    vcs TEXT NOT NULL DEFAULT 'git'
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
    migrate(&conn)?;
    Ok(conn)
}

/// In-place schema migrations for databases created by older versions.
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    // v5: the `vcs` column. New databases already have it (SCHEMA); older
    // databases get it with a 'git' default. Column existence is checked via
    // PRAGMA table_info (the raw ALTER error is a generic SQLITE_ERROR with
    // no dedicated error code), making this migration idempotent.
    let has_vcs = {
        let mut stmt = conn.prepare("PRAGMA table_info(projects)")?;
        let mut rows = stmt.query([])?;
        let mut found = false;
        while let Some(row) = rows.next()? {
            let name: String = row.get(1)?;
            if name == "vcs" {
                found = true;
            }
        }
        found
    };
    if !has_vcs {
        conn.execute(
            "ALTER TABLE projects ADD COLUMN vcs TEXT NOT NULL DEFAULT 'git'",
            [],
        )?;
    }
    Ok(())
}

/// Insert a project and return its row id. Fails if `path` already exists.
///
/// `vcs` is `"git"` or `"svn"` (the string is stored verbatim).
pub fn add_project(
    conn: &Connection,
    name: &str,
    path: &str,
    vcs: &str,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO projects (name, path, vcs) VALUES (?1, ?2, ?3)",
        params![name, path, vcs],
    )?;
    Ok(conn.last_insert_rowid())
}

/// List all projects, oldest first.
pub fn list_projects(conn: &Connection) -> rusqlite::Result<Vec<Project>> {
    let mut stmt =
        conn.prepare("SELECT id, name, path, added_at, vcs FROM projects ORDER BY id ASC")?;
    let rows = stmt.query_map([], |row| {
        Ok(Project {
            id: row.get(0)?,
            name: row.get(1)?,
            path: row.get(2)?,
            added_at: row.get(3)?,
            vcs: row.get(4)?,
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

/// Atomically rotate the master password from `old_password` to `new_password`.
///
/// The verifier and every stored credential are re-encrypted under a fresh
/// random salt and committed in a single transaction. If any step fails, the
/// transaction is rolled back and the vault is left exactly as it was: the old
/// master still unlocks it and every credential stays readable.
///
/// Refuses an empty `new_password` (it would defeat the lock screen).
pub fn change_master_password(
    conn: &Connection,
    old_password: &str,
    new_password: &str,
) -> Result<(), String> {
    if new_password.is_empty() {
        return Err("新密码不能为空".into());
    }

    // Verify the old password; its keys are needed to decrypt the credentials.
    let old_keys = crypto::unlock(conn, old_password).map_err(|err| match err {
        CryptoError::WrongMaster => "旧密码不正确".to_string(),
        other => format!("验证旧密码失败: {other}"),
    })?;

    // Derive the new keys under a fresh salt and re-seal the verifier. Nothing
    // is written yet, so a failure up to here leaves the vault untouched.
    let new_salt = crypto::new_salt();
    let new_keys = crypto::derive_keys(new_password, &new_salt);
    let new_verifier =
        crypto::seal_verifier(&new_keys).map_err(|err| format!("生成新校验值失败: {err}"))?;

    // Everything below runs in one transaction; dropping `tx` on any early
    // return rolls back, keeping the old salt, verifier and ciphertexts.
    let tx = conn
        .unchecked_transaction()
        .map_err(|err| format!("开启事务失败: {err}"))?;

    let mut stmt = tx
        .prepare("SELECT id, nonce, ciphertext FROM credentials")
        .map_err(|err| format!("读取凭据失败: {err}"))?;
    let rows: Vec<(i64, Vec<u8>, Vec<u8>)> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(|err| format!("读取凭据失败: {err}"))?
        .collect::<Result<_, _>>()
        .map_err(|err| format!("读取凭据失败: {err}"))?;
    drop(stmt);

    // Decrypt each secret with the old key and re-seal it with the new one,
    // always under a fresh nonce. Labels and other plaintext columns are not
    // touched.
    let mut reencrypted = Vec::with_capacity(rows.len());
    for (id, nonce, ciphertext) in rows {
        let plaintext = crypto::decrypt(&old_keys, &nonce, &ciphertext)
            .map_err(|err| format!("解密既有凭据失败: {err}"))?;
        let (new_nonce, new_ciphertext) = crypto::encrypt(&new_keys, &plaintext)
            .map_err(|err| format!("重新加密凭据失败: {err}"))?;
        reencrypted.push((id, new_nonce, new_ciphertext));
    }

    tx.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        params![crypto::META_KDF_SALT, &new_salt[..]],
    )
    .map_err(|err| format!("写入新盐失败: {err}"))?;
    tx.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        params![crypto::META_VERIFIER, &new_verifier[..]],
    )
    .map_err(|err| format!("写入新校验值失败: {err}"))?;
    for (id, nonce, ciphertext) in &reencrypted {
        tx.execute(
            "UPDATE credentials SET nonce = ?1, ciphertext = ?2 WHERE id = ?3",
            params![nonce, ciphertext, id],
        )
        .map_err(|err| format!("写回凭据失败: {err}"))?;
    }

    tx.commit().map_err(|err| format!("提交事务失败: {err}"))?;
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
        let id = add_project(&conn, "demo", repo.to_str().unwrap(), "git").unwrap();
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

        let id = add_project(&conn, "demo", repo_path, "git").unwrap();
        assert!(id > 0);

        let projects = list_projects(&conn).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, id);
        assert_eq!(projects[0].name, "demo");
        assert_eq!(projects[0].path, repo_path);
        assert!(!projects[0].added_at.is_empty());
        assert_eq!(projects[0].vcs, "git");
    }

    #[test]
    fn add_and_list_round_trips_vcs() {
        let dir = tempdir().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();
        let repo = dir.path().join("svn-repo");
        fs::create_dir(&repo).unwrap();

        add_project(&conn, "svn-demo", repo.to_str().unwrap(), "svn").unwrap();
        let projects = list_projects(&conn).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].vcs, "svn", "the stored vcs type must round-trip");
    }

    #[test]
    fn raw_rows_without_vcs_default_to_git() {
        let dir = tempdir().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();
        conn.execute(
            "INSERT INTO projects (name, path) VALUES ('legacy', 'C:/x')",
            [],
        )
        .unwrap();

        let projects = list_projects(&conn).unwrap();
        assert_eq!(projects[0].vcs, "git", "rows predating the vcs column are git");
    }

    #[test]
    fn migrate_old_db_without_vcs_column() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("legacy.db");
        // Simulate a v4-era database: the projects table without the vcs column.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE projects (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    path TEXT NOT NULL UNIQUE,
                    added_at TEXT NOT NULL DEFAULT (datetime('now'))
                );
                INSERT INTO projects (name, path) VALUES ('old', 'C:/old');",
            )
            .unwrap();
        }
        // Re-opening through init_db must migrate in place and keep the row.
        let conn = init_db(&db_path).unwrap();
        let projects = list_projects(&conn).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "old");
        assert_eq!(projects[0].vcs, "git");
    }

    #[test]
    fn init_db_is_idempotent() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        init_db(&db_path).unwrap();
        // Second open on an up-to-date schema must not fail on the migration.
        init_db(&db_path).expect("re-init on migrated schema");
    }

    #[test]
    fn duplicate_path_rejected() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let repo_path = repo.to_str().unwrap();

        add_project(&conn, "demo", repo_path, "git").unwrap();
        assert!(add_project(&conn, "other", repo_path, "git").is_err());
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

        let id = add_project(&conn, "demo", repo_path, "git").unwrap();
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
        let other_id = add_project(&conn, "other", repo.to_str().unwrap(), "git").unwrap();
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

    #[test]
    fn change_master_then_unlock_with_new_and_reject_old() {
        let dir = tempdir().unwrap();
        let conn = init_db(&dir.path().join("vault.db")).unwrap();
        crypto::setup_master(&conn, MASTER).unwrap();
        let old_keys = crypto::unlock(&conn, MASTER).unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let project_id = add_project(&conn, "demo", repo.to_str().unwrap(), "git").unwrap();
        store_credential(&conn, &old_keys, project_id, "git_token", "s3cret!").unwrap();
        store_credential(
            &conn,
            &old_keys,
            project_id,
            "db_url",
            "postgres://u:p@h/db",
        )
        .unwrap();

        let salt_before = get_meta(&conn, "kdf_salt").unwrap().unwrap();
        let verifier_before = get_meta(&conn, "verifier").unwrap().unwrap();

        change_master_password(&conn, MASTER, "brand new master").unwrap();

        // A fresh salt and verifier were installed, not just a new verifier.
        assert_ne!(
            get_meta(&conn, "kdf_salt").unwrap().unwrap(),
            salt_before,
            "rotation must install a fresh kdf_salt"
        );
        assert_ne!(
            get_meta(&conn, "verifier").unwrap().unwrap(),
            verifier_before
        );

        // The old master must no longer unlock the vault.
        assert!(matches!(
            crypto::unlock(&conn, MASTER),
            Err(CryptoError::WrongMaster)
        ));

        // The new master unlocks and every credential decrypts verbatim.
        let new_keys = crypto::unlock(&conn, "brand new master").unwrap();
        assert_eq!(
            get_credential(&conn, &new_keys, project_id, "git_token")
                .unwrap()
                .as_deref(),
            Some("s3cret!")
        );
        assert_eq!(
            get_credential(&conn, &new_keys, project_id, "db_url")
                .unwrap()
                .as_deref(),
            Some("postgres://u:p@h/db")
        );
    }

    #[test]
    fn change_master_wrong_old_leaves_vault_intact() {
        let dir = tempdir().unwrap();
        let conn = init_db(&dir.path().join("vault.db")).unwrap();
        crypto::setup_master(&conn, MASTER).unwrap();
        let keys = crypto::unlock(&conn, MASTER).unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let project_id = add_project(&conn, "demo", repo.to_str().unwrap(), "git").unwrap();
        store_credential(&conn, &keys, project_id, "git_token", "s3cret!").unwrap();

        let salt_before = get_meta(&conn, "kdf_salt").unwrap().unwrap();
        let verifier_before = get_meta(&conn, "verifier").unwrap().unwrap();
        let credential = |conn: &Connection| {
            conn.query_row(
                "SELECT nonce, ciphertext FROM credentials WHERE project_id = ?1 AND label = ?2",
                params![project_id, "git_token"],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .unwrap()
        };
        let credential_before = credential(&conn);

        let err =
            change_master_password(&conn, "not the old password", "brand new master").unwrap_err();
        assert_eq!(err, "旧密码不正确");

        // Nothing changed: same salt, verifier and ciphertext blobs...
        assert_eq!(get_meta(&conn, "kdf_salt").unwrap().unwrap(), salt_before);
        assert_eq!(
            get_meta(&conn, "verifier").unwrap().unwrap(),
            verifier_before
        );
        assert_eq!(credential(&conn), credential_before);

        // ...and the old master still unlocks and decrypts everything.
        let keys_after = crypto::unlock(&conn, MASTER).unwrap();
        assert_eq!(
            get_credential(&conn, &keys_after, project_id, "git_token")
                .unwrap()
                .as_deref(),
            Some("s3cret!")
        );
    }

    #[test]
    fn change_master_empty_new_password_is_err() {
        let dir = tempdir().unwrap();
        let conn = init_db(&dir.path().join("vault.db")).unwrap();
        crypto::setup_master(&conn, MASTER).unwrap();
        let keys = crypto::unlock(&conn, MASTER).unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let project_id = add_project(&conn, "demo", repo.to_str().unwrap(), "git").unwrap();
        store_credential(&conn, &keys, project_id, "git_token", "s3cret!").unwrap();

        assert!(change_master_password(&conn, MASTER, "").is_err());

        // The vault is untouched: the old master still unlocks and decrypts.
        let keys_after = crypto::unlock(&conn, MASTER).unwrap();
        assert_eq!(
            get_credential(&conn, &keys_after, project_id, "git_token")
                .unwrap()
                .as_deref(),
            Some("s3cret!")
        );
    }
}
