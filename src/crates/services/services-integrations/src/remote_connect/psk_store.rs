//! Encrypted local storage for out-of-band device pairing secrets (PSK).
//!
//! A random 32-byte key lives in `<relay-home>/.psk_vault.key` (0600 on Unix).
//! The ciphertext map is stored in `<relay-home>/psk_vault.json`; each entry is
//! AES-256-GCM encrypted and keyed by SHA-256 of
//! `(relay_url, user_id, peer_device_id)`. Read-modify-write cycles hold an
//! exclusive lock file so Desktop and CLI processes sharing one home stay
//! consistent. Pairing secrets never leave this machine through the relay.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;

use super::device_crypto;

const NONCE_LEN: usize = 12;

/// Lifecycle of a stored pairing secret for one peer device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerPskStatus {
    /// This device generated the pairing code and waits for the peer's
    /// confirmation. Traffic still uses the legacy derivation until then.
    Offered,
    /// Both devices hold the secret; traffic uses the PSK derivation.
    Paired,
}

/// A decrypted pairing entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerPskEntry {
    pub peer_device_id: String,
    pub psk: [u8; 32],
    pub psk_id: String,
    pub status: PeerPskStatus,
    pub created_at_unix: u64,
    pub peer_name: Option<String>,
}

/// Listing projection without the secret itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerPskListing {
    pub peer_device_id: String,
    pub psk_id: String,
    pub status: PeerPskStatus,
    pub created_at_unix: u64,
    pub peer_name: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct VaultFile {
    entries: HashMap<String, String>,
}

fn entry_key(relay_url: &str, user_id: &str, peer_device_id: &str) -> String {
    let digest = Sha256::new()
        .chain_update(relay_url.as_bytes())
        .chain_update(user_id.as_bytes())
        .chain_update(peer_device_id.as_bytes())
        .finalize();
    hex::encode(digest)
}

fn vault_paths() -> Result<(PathBuf, PathBuf, PathBuf)> {
    let dir = super::session_store::data_directory()?;
    Ok((
        dir.join(".psk_vault.key"),
        dir.join("psk_vault.json"),
        dir.join("psk_vault.lock"),
    ))
}

/// Hold the cross-process lock while reading or writing the vault.
fn with_vault_lock<T>(run: impl FnOnce() -> Result<T>) -> Result<T> {
    let (_, _, lock_path) = vault_paths()?;
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .context("open psk vault lock")?;
    fs2::FileExt::lock_exclusive(&file).context("lock psk vault")?;
    let result = run();
    let _ = fs2::FileExt::unlock(&file);
    result
}

fn ensure_key(key_path: &PathBuf) -> Result<[u8; 32]> {
    if key_path.exists() {
        let bytes = std::fs::read(key_path).context("read psk vault key")?;
        if bytes.len() != 32 {
            anyhow::bail!("invalid psk vault key length");
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        return Ok(key);
    }
    if let Some(parent) = key_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut key = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut key);
    crate::remote_persistence::write_private_bytes(key_path, &key)?;
    Ok(key)
}

fn encrypt_entry(key: &[u8; 32], entry: &PeerPskEntry) -> Result<String> {
    #[derive(Serialize)]
    struct EntryBody<'a> {
        peer_device_id: &'a str,
        psk: &'a str,
        psk_id: &'a str,
        status: PeerPskStatus,
        created_at_unix: u64,
        peer_name: Option<&'a str>,
    }
    let psk = B64.encode(entry.psk);
    let body = EntryBody {
        peer_device_id: &entry.peer_device_id,
        psk: &psk,
        psk_id: &entry.psk_id,
        status: entry.status,
        created_at_unix: entry.created_at_unix,
        peer_name: entry.peer_name.as_deref(),
    };
    let plaintext = serde_json::to_vec(&body)?;
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut nonce = [0u8; NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext.as_slice())
        .map_err(|e| anyhow::anyhow!("encrypt psk entry: {e}"))?;
    let mut blob = Vec::with_capacity(NONCE_LEN + ct.len());
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ct);
    Ok(B64.encode(blob))
}

fn decrypt_entry(key: &[u8; 32], blob_b64: &str) -> Result<PeerPskEntry> {
    #[derive(Deserialize)]
    struct EntryBody {
        #[serde(default)]
        peer_device_id: Option<String>,
        psk: String,
        psk_id: String,
        status: PeerPskStatus,
        created_at_unix: u64,
        peer_name: Option<String>,
    }
    let blob = B64.decode(blob_b64).context("base64 decode psk entry")?;
    if blob.len() <= NONCE_LEN {
        anyhow::bail!("psk vault entry too short");
    }
    let (nonce, ct) = blob.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| anyhow::anyhow!("{e}"))?;
    let pt = cipher
        .decrypt(Nonce::from_slice(nonce), ct)
        .map_err(|e| anyhow::anyhow!("decrypt psk entry: {e}"))?;
    let body: EntryBody = serde_json::from_slice(&pt)?;
    let psk_bytes = B64.decode(&body.psk).context("base64 decode psk")?;
    if psk_bytes.len() != 32 {
        anyhow::bail!("invalid psk length in vault entry");
    }
    let mut psk = [0u8; 32];
    psk.copy_from_slice(&psk_bytes);
    Ok(PeerPskEntry {
        // Entries written before the field existed lose their listing row but
        // keep working for key derivation through `lookup_entry`.
        peer_device_id: body.peer_device_id.unwrap_or_default(),
        psk,
        psk_id: body.psk_id,
        status: body.status,
        created_at_unix: body.created_at_unix,
        peer_name: body.peer_name,
    })
}

fn read_vault(vault_path: &PathBuf) -> VaultFile {
    if !vault_path.exists() {
        return VaultFile::default();
    }
    let data = std::fs::read_to_string(vault_path).unwrap_or_default();
    serde_json::from_str(&data).unwrap_or_default()
}

fn write_vault(vault_path: &PathBuf, file: &VaultFile) -> Result<()> {
    if file.entries.is_empty() {
        let _ = std::fs::remove_file(vault_path);
        return Ok(());
    }
    let body = serde_json::to_string_pretty(file)?;
    crate::remote_persistence::write_private_bytes(vault_path, body.as_bytes())
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── Public API ──────────────────────────────────────────────────────────

/// Store or replace the pairing secret for one peer, keeping the caller's
/// lifecycle status (offerer stores `Offered`, completer stores `Paired`).
pub fn store_entry(
    relay_url: &str,
    user_id: &str,
    peer_device_id: &str,
    psk: &[u8; 32],
    status: PeerPskStatus,
    peer_name: Option<&str>,
) -> Result<PeerPskEntry> {
    let entry = PeerPskEntry {
        peer_device_id: peer_device_id.to_string(),
        psk: *psk,
        psk_id: device_crypto::psk_key_id(psk),
        status,
        created_at_unix: now_unix(),
        peer_name: peer_name.map(str::to_string),
    };
    with_vault_lock(|| {
        let (key_path, vault_path, _) = vault_paths()?;
        let key = ensure_key(&key_path)?;
        let mut file = read_vault(&vault_path);
        file.entries.insert(
            entry_key(relay_url, user_id, peer_device_id),
            encrypt_entry(&key, &entry)?,
        );
        write_vault(&vault_path, &file)
    })?;
    Ok(entry)
}

/// Load the pairing secret for one peer, if this device holds one.
pub fn lookup_entry(
    relay_url: &str,
    user_id: &str,
    peer_device_id: &str,
) -> Result<Option<PeerPskEntry>> {
    with_vault_lock(|| {
        let (key_path, vault_path, _) = vault_paths()?;
        if !vault_path.exists() || !key_path.exists() {
            return Ok(None);
        }
        let key = ensure_key(&key_path)?;
        let file = read_vault(&vault_path);
        let Some(blob) = file
            .entries
            .get(&entry_key(relay_url, user_id, peer_device_id))
        else {
            return Ok(None);
        };
        match decrypt_entry(&key, blob) {
            Ok(entry) => Ok(Some(entry)),
            Err(error) => {
                log::warn!(
                    "Failed to decrypt psk vault entry for device {}: {}",
                    peer_device_id,
                    error
                );
                Ok(None)
            }
        }
    })
}

/// Promote a stored `Offered` (or already `Paired`) entry to `Paired` when the
/// confirmed PSK id matches. Returns false when no compatible entry exists.
pub fn mark_paired(
    relay_url: &str,
    user_id: &str,
    peer_device_id: &str,
    psk_id: &str,
) -> Result<bool> {
    with_vault_lock(|| {
        let (key_path, vault_path, _) = vault_paths()?;
        if !vault_path.exists() || !key_path.exists() {
            return Ok(false);
        }
        let key = ensure_key(&key_path)?;
        let mut file = read_vault(&vault_path);
        let entry_key = entry_key(relay_url, user_id, peer_device_id);
        let Some(blob) = file.entries.get(&entry_key) else {
            return Ok(false);
        };
        let mut entry = match decrypt_entry(&key, blob) {
            Ok(entry) => entry,
            Err(error) => {
                log::warn!(
                    "Failed to decrypt psk vault entry for device {peer_device_id}: {error}"
                );
                return Ok(false);
            }
        };
        if entry.psk_id != psk_id {
            return Ok(false);
        }
        entry.status = PeerPskStatus::Paired;
        file.entries.insert(entry_key, encrypt_entry(&key, &entry)?);
        write_vault(&vault_path, &file)?;
        Ok(true)
    })
}

/// Forget the pairing secret for one peer. Returns true when an entry existed.
pub fn remove_entry(relay_url: &str, user_id: &str, peer_device_id: &str) -> Result<bool> {
    with_vault_lock(|| {
        let (_, vault_path, _) = vault_paths()?;
        if !vault_path.exists() {
            return Ok(false);
        }
        let mut file = read_vault(&vault_path);
        let removed = file
            .entries
            .remove(&entry_key(relay_url, user_id, peer_device_id))
            .is_some();
        write_vault(&vault_path, &file)?;
        Ok(removed)
    })
}

/// List every pairing entry for one relay account, without exposing secrets.
pub fn list_entries(relay_url: &str, user_id: &str) -> Result<Vec<PeerPskListing>> {
    with_vault_lock(|| {
        let (key_path, vault_path, _) = vault_paths()?;
        if !vault_path.exists() || !key_path.exists() {
            return Ok(Vec::new());
        }
        let key = ensure_key(&key_path)?;
        let file = read_vault(&vault_path);
        let mut listings = Vec::new();
        for blob in file.entries.values() {
            match decrypt_entry(&key, blob) {
                Ok(entry) if !entry.peer_device_id.is_empty() => listings.push(PeerPskListing {
                    peer_device_id: entry.peer_device_id,
                    psk_id: entry.psk_id,
                    status: entry.status,
                    created_at_unix: entry.created_at_unix,
                    peer_name: entry.peer_name,
                }),
                Ok(_) => {}
                Err(error) => {
                    log::warn!("Failed to decrypt psk vault entry while listing: {error}");
                }
            }
        }
        listings.sort_by(|a, b| a.peer_device_id.cmp(&b.peer_device_id));
        Ok(listings)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir() -> PathBuf {
        let dir = tempfile::tempdir().unwrap().keep();
        let _store_guard = super::super::session_store::test_store_guard()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        super::super::session_store::set_session_store_directory_for_test(dir.clone());
        dir
    }

    #[test]
    fn entries_round_trip_and_remove() {
        test_dir();
        let psk = [77u8; 32];
        let entry = store_entry(
            "https://relay.example",
            "user-1",
            "peer-1",
            &psk,
            PeerPskStatus::Offered,
            Some("Work laptop"),
        )
        .unwrap();
        assert_eq!(entry.status, PeerPskStatus::Offered);
        assert_eq!(entry.psk_id, device_crypto::psk_key_id(&psk));

        let loaded = lookup_entry("https://relay.example", "user-1", "peer-1")
            .unwrap()
            .expect("entry exists");
        assert_eq!(loaded.psk, psk);
        assert_eq!(loaded.status, PeerPskStatus::Offered);
        assert_eq!(loaded.peer_name.as_deref(), Some("Work laptop"));

        // Account or peer scoping must not cross.
        assert!(lookup_entry("https://relay.example", "user-2", "peer-1")
            .unwrap()
            .is_none());
        assert!(lookup_entry("https://relay.example", "user-1", "peer-2")
            .unwrap()
            .is_none());

        assert!(mark_paired("https://relay.example", "user-1", "peer-1", &entry.psk_id).unwrap());
        let paired = lookup_entry("https://relay.example", "user-1", "peer-1")
            .unwrap()
            .unwrap();
        assert_eq!(paired.status, PeerPskStatus::Paired);
        // A mismatched id must not promote anything.
        assert!(!mark_paired(
            "https://relay.example",
            "user-1",
            "peer-1",
            "0000000000000000"
        )
        .unwrap());

        let listings = list_entries("https://relay.example", "user-1").unwrap();
        assert_eq!(listings.len(), 1);
        assert_eq!(listings[0].peer_device_id, "peer-1");
        assert_eq!(listings[0].status, PeerPskStatus::Paired);

        assert!(remove_entry("https://relay.example", "user-1", "peer-1").unwrap());
        assert!(!remove_entry("https://relay.example", "user-1", "peer-1").unwrap());
        assert!(lookup_entry("https://relay.example", "user-1", "peer-1")
            .unwrap()
            .is_none());
    }
}
