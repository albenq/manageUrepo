//! Master-password key derivation and authenticated encryption for stored
//! credentials.
//!
//! The vault salt and a verifier blob live in the `meta` table:
//! - `kdf_salt`: 16 random bytes,
//! - `verifier`: `nonce (12 bytes) || ciphertext` of the magic string, sealed
//!   with the derived key.
//!
//! This module is pure domain code and must not depend on egui.

use std::fmt;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::rngs::OsRng;
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};

const META_KDF_SALT: &str = "kdf_salt";
const META_VERIFIER: &str = "verifier";

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const MAGIC: &[u8] = b"git-pm-vault-v1";

const ARGON2_M_COST: u32 = 65536; // 64 MiB
const ARGON2_T_COST: u32 = 3;
const ARGON2_P_COST: u32 = 1;

/// Keys derived from the master password.
#[derive(Clone)]
pub struct VaultKeys {
    pub enc_key: [u8; 32],
}

#[derive(Debug)]
pub enum CryptoError {
    WrongMaster,
    Database(String),
    Random,
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CryptoError::WrongMaster => write!(f, "wrong master password"),
            CryptoError::Database(msg) => write!(f, "vault database error: {msg}"),
            CryptoError::Random => write!(f, "randomness or encryption failure"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// Derive the 32-byte encryption key with Argon2id (m=64 MiB, t=3, p=1).
///
/// `salt` should be [`SALT_LEN`] bytes; 16-byte salts are what setup stores.
pub fn derive_keys(master: &str, salt: &[u8]) -> VaultKeys {
    let params = Params::new(ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST, Some(32))
        .expect("hard-coded Argon2 parameters are valid");
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut enc_key = [0u8; 32];
    argon2
        .hash_password_into(master.as_bytes(), salt, &mut enc_key)
        .expect("Argon2id derivation with a 16-byte salt cannot fail");
    VaultKeys { enc_key }
}

/// Generate a fresh salt, encrypt the magic verifier with the master-derived
/// key, and persist both in `meta`.
///
/// Refuses to run on an already-initialized vault: rotating the salt would make
/// every ciphertext sealed under the old key permanently undecryptable.
pub fn setup_master(conn: &Connection, master: &str) -> Result<(), CryptoError> {
    if meta_exists(conn, META_VERIFIER)? {
        return Err(CryptoError::Database(
            "vault is already initialized; refusing to rotate kdf_salt".into(),
        ));
    }

    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);

    let keys = derive_keys(master, &salt);
    let (nonce, ciphertext) = encrypt(&keys, MAGIC)?;
    let mut verifier = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    verifier.extend_from_slice(&nonce);
    verifier.extend_from_slice(&ciphertext);

    let tx = conn
        .unchecked_transaction()
        .map_err(|e| CryptoError::Database(e.to_string()))?;
    tx.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        params![META_KDF_SALT, &salt[..]],
    )
    .map_err(|e| CryptoError::Database(e.to_string()))?;
    tx.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        params![META_VERIFIER, verifier],
    )
    .map_err(|e| CryptoError::Database(e.to_string()))?;
    tx.commit().map_err(|e| CryptoError::Database(e.to_string()))?;
    Ok(())
}

/// Derive keys from `master` and check them against the stored verifier.
pub fn unlock(conn: &Connection, master: &str) -> Result<VaultKeys, CryptoError> {
    let salt = get_meta(conn, META_KDF_SALT)?;
    let verifier = get_meta(conn, META_VERIFIER)?;
    if salt.len() != SALT_LEN {
        return Err(CryptoError::Database(format!(
            "malformed kdf_salt: expected {SALT_LEN} bytes, found {}",
            salt.len()
        )));
    }
    if verifier.len() <= NONCE_LEN {
        return Err(CryptoError::Database("malformed verifier blob".into()));
    }

    let keys = derive_keys(master, &salt);
    let (nonce, ciphertext) = verifier.split_at(NONCE_LEN);
    let plaintext = decrypt(&keys, nonce, ciphertext)?;
    if plaintext == MAGIC {
        Ok(keys)
    } else {
        Err(CryptoError::WrongMaster)
    }
}

/// Encrypt `plaintext` with a fresh 12-byte random nonce, returning
/// `(nonce, ciphertext)`.
pub fn encrypt(key: &VaultKeys, plaintext: &[u8]) -> Result<(Vec<u8>, Vec<u8>), CryptoError> {
    let cipher = Aes256Gcm::new_from_slice(&key.enc_key)
        .map_err(|_| CryptoError::Database("invalid key length".into()))?;

    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);

    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plaintext)
        .map_err(|_| CryptoError::Random)?;
    Ok((nonce_bytes.to_vec(), ciphertext))
}

/// Decrypt `ciphertext`; any AES-GCM failure (wrong key, tampering, bad nonce
/// length) maps to [`CryptoError::WrongMaster`].
pub fn decrypt(key: &VaultKeys, nonce: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if nonce.len() != NONCE_LEN {
        return Err(CryptoError::WrongMaster);
    }
    let cipher = Aes256Gcm::new_from_slice(&key.enc_key).map_err(|_| CryptoError::WrongMaster)?;
    cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| CryptoError::WrongMaster)
}

fn get_meta(conn: &Connection, key: &str) -> Result<Vec<u8>, CryptoError> {
    conn.query_row(
        "SELECT value FROM meta WHERE key = ?1",
        params![key],
        |row| row.get::<_, Vec<u8>>(0),
    )
    .optional()
    .map_err(|e| CryptoError::Database(e.to_string()))?
    .ok_or_else(|| CryptoError::Database(format!("missing meta key `{key}`")))
}

fn meta_exists(conn: &Connection, key: &str) -> Result<bool, CryptoError> {
    conn.query_row("SELECT 1 FROM meta WHERE key = ?1", params![key], |_| Ok(()))
        .optional()
        .map(|present| present.is_some())
        .map_err(|e| CryptoError::Database(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;
    use tempfile::tempdir;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let keys = VaultKeys { enc_key: [7u8; 32] };
        let plaintext = b"hello vault";

        let (nonce1, ct1) = encrypt(&keys, plaintext).unwrap();
        let (nonce2, ct2) = encrypt(&keys, plaintext).unwrap();

        assert_eq!(nonce1.len(), 12);
        assert_eq!(nonce2.len(), 12);
        assert_ne!(nonce1, nonce2, "nonce must be random per encryption");
        assert_ne!(ct1, plaintext);
        assert_ne!(ct2, plaintext);

        let restored = decrypt(&keys, &nonce1, &ct1).unwrap();
        assert_eq!(restored, plaintext);
        let restored2 = decrypt(&keys, &nonce2, &ct2).unwrap();
        assert_eq!(restored2, plaintext);
    }

    #[test]
    fn setup_then_unlock_ok() {
        let dir = tempdir().unwrap();
        let conn = init_db(&dir.path().join("vault.db")).unwrap();

        setup_master(&conn, "correct horse").unwrap();
        let keys = unlock(&conn, "correct horse").unwrap();

        // The stored verifier must decrypt back to the magic string with these keys.
        let verifier: Vec<u8> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params!["verifier"],
                |row| row.get(0),
            )
            .unwrap();
        assert!(verifier.len() > 12);
        let (nonce, ct) = verifier.split_at(12);
        assert_eq!(decrypt(&keys, nonce, ct).unwrap(), MAGIC);

        // Salt is 16 random bytes.
        let salt: Vec<u8> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params!["kdf_salt"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(salt.len(), 16);
    }

    #[test]
    fn wrong_master_fails() {
        let dir = tempdir().unwrap();
        let conn = init_db(&dir.path().join("vault.db")).unwrap();

        setup_master(&conn, "right password").unwrap();
        let err = match unlock(&conn, "wrong password") {
            Ok(_) => panic!("wrong master must not unlock"),
            Err(e) => e,
        };
        assert!(matches!(err, CryptoError::WrongMaster));
    }

    #[test]
    fn tampered_salt_is_database_error() {
        let dir = tempdir().unwrap();
        let conn = init_db(&dir.path().join("vault.db")).unwrap();

        setup_master(&conn, "correct horse").unwrap();
        // Simulate a truncated / corrupted salt in the database.
        conn.execute(
            "UPDATE meta SET value = ?1 WHERE key = ?2",
            params![&[0xABu8; 4][..], "kdf_salt"],
        )
        .unwrap();

        // Must report a Database error instead of panicking inside derive_keys.
        let err = match unlock(&conn, "correct horse") {
            Ok(_) => panic!("corrupted salt must not unlock"),
            Err(e) => e,
        };
        assert!(
            matches!(err, CryptoError::Database(_)),
            "expected Database error, got {err}"
        );
    }

    #[test]
    fn second_setup_is_rejected() {
        let dir = tempdir().unwrap();
        let conn = init_db(&dir.path().join("vault.db")).unwrap();

        setup_master(&conn, "first master").unwrap();
        let salt_before = get_meta(&conn, "kdf_salt").unwrap();
        let verifier_before = get_meta(&conn, "verifier").unwrap();

        let err = match setup_master(&conn, "second master") {
            Ok(()) => panic!("re-running setup must be rejected"),
            Err(e) => e,
        };
        assert!(
            matches!(err, CryptoError::Database(_)),
            "expected Database error, got {err}"
        );

        // Existing material must be untouched and the original master still works.
        assert_eq!(get_meta(&conn, "kdf_salt").unwrap(), salt_before);
        assert_eq!(get_meta(&conn, "verifier").unwrap(), verifier_before);
        unlock(&conn, "first master").unwrap();
        assert!(matches!(
            unlock(&conn, "second master"),
            Err(CryptoError::WrongMaster)
        ));
    }
}
