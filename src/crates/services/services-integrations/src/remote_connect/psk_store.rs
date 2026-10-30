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

fn entry_key(relay_url: &str, user_id: &str, peer_device_id: &str) -> Result<String> {
    let peer = super::device_grants::PeerTarget::parse(peer_device_id)?;
    if peer.is_shared() {
        let tuple = serde_json::to_vec(&(
            "psk-peer-v1",
            relay_url.trim_end_matches('/'),
            user_id,
            peer.account_id,
            peer.device_id,
        ))?;
        return Ok(hex::encode(Sha256::digest(tuple)));
    }
    let digest = Sha256::new()
        .chain_update(relay_url.as_bytes())
        .chain_update(user_id.as_bytes())
        .chain_update(peer_device_id.as_bytes())
        .finalize();
    Ok(hex::encode(digest))
}

/// A cross-account pairing offer is minted by the shared host before it learns
/// the grantee's controller device, so it is keyed by the grant id rather than
/// a device. The grantee's first PSK-sealed message (whose relay-derived source
/// account resolves back to this grant) promotes it to a device-scoped entry.
fn grant_offer_key(relay_url: &str, owner_user_id: &str, grant_id: &str) -> String {
    let tuple = serde_json::to_vec(&(
        "psk-grant-offer-v1",
        relay_url.trim_end_matches('/'),
        owner_user_id,
        grant_id,
    ))
    .expect("grant offer key tuple");
    hex::encode(Sha256::digest(tuple))
}

/// Store a pending cross-account pairing offer under its grant id.
pub fn store_grant_offer(
    relay_url: &str,
    owner_user_id: &str,
    grant_id: &str,
    psk: &[u8; 32],
    peer_name: Option<&str>,
) -> Result<PeerPskEntry> {
    let entry = PeerPskEntry {
        peer_device_id: String::new(),
        psk: *psk,
        psk_id: device_crypto::psk_key_id(psk),
        status: PeerPskStatus::Offered,
        created_at_unix: now_unix(),
        peer_name: peer_name.map(str::to_string),
    };
    with_vault_lock(|| {
        let (key_path, vault_path, _) = vault_paths()?;
        let key = ensure_key(&key_path)?;
        let mut file = read_vault(&vault_path)?;
        file.entries.insert(
            grant_offer_key(relay_url, owner_user_id, grant_id),
            encrypt_entry(&key, &entry)?,
        );
        write_vault(&vault_path, &file)
    })?;
    Ok(entry)
}

/// Load the pending pairing offer for one grant, if the host minted one.
pub fn lookup_grant_offer(
    relay_url: &str,
    owner_user_id: &str,
    grant_id: &str,
) -> Result<Option<PeerPskEntry>> {
    with_vault_lock(|| {
        let (key_path, vault_path, _) = vault_paths()?;
        if !vault_path.exists() || !key_path.exists() {
            return Ok(None);
        }
        let key = ensure_key(&key_path)?;
        let file = read_vault(&vault_path)?;
        let Some(blob) = file
            .entries
            .get(&grant_offer_key(relay_url, owner_user_id, grant_id))
        else {
            return Ok(None);
        };
        match decrypt_entry(&key, blob) {
            Ok(entry) => Ok(Some(entry)),
            Err(error) => {
                log::warn!("Failed to decrypt grant pairing offer for grant {grant_id}: {error}");
                Ok(None)
            }
        }
    })
}

/// Promote a grant's pending offer to a device-scoped paired entry when the
/// confirming PSK id matches, then drop the grant-keyed offer. The device-scoped
/// entry uses the shared [`PeerTarget`] so the pairing survives grant renewal.
pub fn promote_grant_offer(
    relay_url: &str,
    owner_user_id: &str,
    grant_id: &str,
    peer_key: &str,
    psk_id: &str,
) -> Result<bool> {
    with_vault_lock(|| {
        let (key_path, vault_path, _) = vault_paths()?;
        if !vault_path.exists() || !key_path.exists() {
            return Ok(false);
        }
        let key = ensure_key(&key_path)?;
        let mut file = read_vault(&vault_path)?;
        let offer_key = grant_offer_key(relay_url, owner_user_id, grant_id);
        let Some(blob) = file.entries.get(&offer_key) else {
            return Ok(false);
        };
        let Ok(mut entry) = decrypt_entry(&key, blob) else {
            return Ok(false);
        };
        if entry.psk_id != psk_id {
            return Ok(false);
        }
        entry.peer_device_id = super::device_grants::PeerTarget::parse(peer_key)?.device_id;
        entry.status = PeerPskStatus::Paired;
        file.entries.remove(&offer_key);
        file.entries.insert(
            entry_key(relay_url, owner_user_id, peer_key)?,
            encrypt_entry(&key, &entry)?,
        );
        write_vault(&vault_path, &file)?;
        Ok(true)
    })
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
    anyhow::ensure!(
        !key_path.with_file_name("psk_vault.json").exists(),
        "pairing vault key is missing; existing pairings were preserved"
    );
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

fn read_vault(vault_path: &PathBuf) -> Result<VaultFile> {
    match std::fs::read(vault_path) {
        Ok(data) => serde_json::from_slice(&data)
            .context("parse pairing vault; existing data was preserved"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(VaultFile::default()),
        Err(error) => Err(error).context("read pairing vault"),
    }
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
        let mut file = read_vault(&vault_path)?;
        file.entries.insert(
            entry_key(relay_url, user_id, peer_device_id)?,
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
        let file = read_vault(&vault_path)?;
        let Some(blob) = file
            .entries
            .get(&entry_key(relay_url, user_id, peer_device_id)?)
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
        let mut file = read_vault(&vault_path)?;
        let entry_key = entry_key(relay_url, user_id, peer_device_id)?;
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
        let mut file = read_vault(&vault_path)?;
        let removed = file
            .entries
            .remove(&entry_key(relay_url, user_id, peer_device_id)?)
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
        let file = read_vault(&vault_path)?;
        let mut listings = Vec::new();
        for (stored_key, blob) in &file.entries {
            match decrypt_entry(&key, blob) {
                Ok(entry)
                    if !entry.peer_device_id.is_empty()
                        && entry_key(relay_url, user_id, &entry.peer_device_id)
                            .as_ref()
                            .ok()
                            == Some(stored_key) =>
                {
                    listings.push(PeerPskListing {
                        peer_device_id: entry.peer_device_id,
                        psk_id: entry.psk_id,
                        status: entry.status,
                        created_at_unix: entry.created_at_unix,
                        peer_name: entry.peer_name,
                    })
                }
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

// ── Pairing enforcement policy ──────────────────────────────────────────

/// Refusal text when the enforcement policy blocks an unpaired controller.
pub const PAIRING_REQUIRED_MESSAGE: &str = "This host requires PSK pairing before it can be controlled. Pair with this device first (device switcher > pair, or `openbitfun peer pair`).";

/// Commands an unpaired controller may still send under enforcement: the
/// capability probe and the pairing handshake itself. Everything else —
/// attach, product HostInvoke, stream reads — is refused until paired.
pub const PAIRING_EXEMPT_COMMANDS: &[&str] = &["peer_mode_ping", "peer_psk_confirm"];

/// Local, host-side control policy. When enabled, the host refuses product
/// control from peer devices that do not hold a confirmed pairing, so an
/// unpaired (or directory-only-trusting) controller cannot drive or read this
/// machine. Stored beside the vault; deliberately not part of the synced
/// account config — enforcement is a property of the machine being protected.
///
/// Enforcement defaults to ON: a fresh or upgraded host only lets paired
/// devices control it. The pairing handshake itself stays reachable, so a
/// controller can always pair first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingPolicy {
    #[serde(default = "default_require_paired_controller")]
    pub require_paired_controller: bool,
}

fn default_require_paired_controller() -> bool {
    true
}

impl Default for PairingPolicy {
    fn default() -> Self {
        Self {
            require_paired_controller: true,
        }
    }
}

fn policy_path() -> Result<PathBuf> {
    Ok(super::session_store::data_directory()?.join("pairing_policy.json"))
}

/// Read the local enforcement policy; defaults to permissive for machines
/// that never wrote the file.
pub fn load_policy() -> Result<PairingPolicy> {
    let path = policy_path()?;
    if !path.exists() {
        return Ok(PairingPolicy::default());
    }
    let data = std::fs::read_to_string(&path).context("read pairing policy")?;
    serde_json::from_str(&data).context("parse pairing policy")
}

/// Persist the local enforcement policy.
pub fn store_policy(policy: &PairingPolicy) -> Result<()> {
    let path = policy_path()?;
    let body = serde_json::to_string_pretty(policy)?;
    crate::remote_persistence::write_private_bytes(&path, body.as_bytes())
}

/// Whether `peer_device_id` may control this host: the policy is off, or the
/// device holds a confirmed pairing. `PeerPskStatus::Offered` does not count —
/// an offer is this machine's intent, not a verified shared secret.
pub fn controller_meets_pairing_policy(
    relay_url: &str,
    user_id: &str,
    peer_device_id: &str,
) -> Result<bool> {
    if !load_policy()?.require_paired_controller {
        return Ok(true);
    }
    Ok(lookup_entry(relay_url, user_id, peer_device_id)?
        .is_some_and(|entry| entry.status == PeerPskStatus::Paired))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `test_dir()` repoints a process-global session-store directory, so the
    /// two tests that exercise the on-disk vault must not interleave.
    static STORE_TEST_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn enforcement_policy_persists_and_gates_controllers() {
        let _guard = STORE_TEST_SERIAL.lock().unwrap();
        test_dir();
        // Enforcement defaults to ON: a host only lets paired devices control
        // it, and an unreadable/legacy policy file resolves to the same.
        assert_eq!(load_policy().unwrap().require_paired_controller, true);
        assert!(!controller_meets_pairing_policy("r", "u", "peer-1").unwrap());
        // The user can turn enforcement off for this machine.
        store_policy(&PairingPolicy {
            require_paired_controller: false,
        })
        .unwrap();
        assert_eq!(load_policy().unwrap().require_paired_controller, false);
        assert!(controller_meets_pairing_policy("r", "u", "any").unwrap());

        store_policy(&PairingPolicy {
            require_paired_controller: true,
        })
        .unwrap();
        assert_eq!(load_policy().unwrap().require_paired_controller, true);
        // Enforcement on: an unpaired controller is refused, an offered one
        // still is (an offer is intent, not proof), only paired passes.
        assert!(!controller_meets_pairing_policy("r", "u", "peer-1").unwrap());
        store_entry(
            "r",
            "u",
            "peer-1",
            &[91u8; 32],
            PeerPskStatus::Offered,
            None,
        )
        .unwrap();
        assert!(!controller_meets_pairing_policy("r", "u", "peer-1").unwrap());
        store_entry("r", "u", "peer-2", &[92u8; 32], PeerPskStatus::Paired, None).unwrap();
        assert!(controller_meets_pairing_policy("r", "u", "peer-2").unwrap());
        // A different account scope does not inherit the pairing.
        assert!(!controller_meets_pairing_policy("r", "other", "peer-2").unwrap());
    }

    #[test]
    fn cross_account_keys_are_disjoint_and_legacy_keys_stay_stable() {
        use super::super::device_grants::PeerTarget;
        let a = PeerTarget::shared("owner-a", "device", "grant-a").key();
        let b = PeerTarget::shared("owner-b", "device", "grant-b").key();
        let renewed = PeerTarget::shared("owner-a", "device", "new-grant").key();
        assert_ne!(
            entry_key("r", "me", &a).unwrap(),
            entry_key("r", "me", &b).unwrap()
        );
        assert_eq!(
            entry_key("r", "me", &a).unwrap(),
            entry_key("r", "me", &renewed).unwrap()
        );
        assert_ne!(
            entry_key("r", "me", &a).unwrap(),
            entry_key("r", "me", "device").unwrap()
        );
        assert_eq!(
            entry_key("r", "me", "device").unwrap(),
            hex::encode(Sha256::digest(b"rmedevice"))
        );
    }

    #[test]
    fn grant_offer_promotes_to_device_scoped_pairing() {
        let _serial = STORE_TEST_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        test_dir();
        use super::super::device_grants::PeerTarget;
        let psk = [63u8; 32];
        let offer = store_grant_offer("https://relay", "owner", "g1", &psk, Some("shared"))
            .expect("store grant offer");
        assert_eq!(offer.status, PeerPskStatus::Offered);
        assert!(lookup_grant_offer("https://relay", "owner", "g1")
            .unwrap()
            .is_some());
        // A wrong grant is invisible.
        assert!(lookup_grant_offer("https://relay", "owner", "g2")
            .unwrap()
            .is_none());
        let grantee = PeerTarget::shared("member", "member-desk", "g1");
        // Mismatched psk id does not promote.
        assert!(
            !promote_grant_offer("https://relay", "owner", "g1", &grantee.key(), "deadbeef")
                .unwrap()
        );
        assert!(promote_grant_offer(
            "https://relay",
            "owner",
            "g1",
            &grantee.key(),
            &offer.psk_id
        )
        .unwrap());
        // The grant-keyed offer is consumed; the device-scoped entry is paired.
        assert!(lookup_grant_offer("https://relay", "owner", "g1")
            .unwrap()
            .is_none());
        let paired = lookup_entry("https://relay", "owner", &grantee.key())
            .unwrap()
            .expect("paired entry");
        assert_eq!(paired.status, PeerPskStatus::Paired);
        assert_eq!(paired.psk, psk);
        // Renewing the grant keeps the pairing (identity is account+device).
        let renewed = PeerTarget::shared("member", "member-desk", "g-new");
        assert_eq!(
            entry_key("https://relay", "owner", &renewed.key()).unwrap(),
            entry_key("https://relay", "owner", &grantee.key()).unwrap()
        );
    }

    #[test]
    fn corrupted_vault_is_not_an_empty_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("psk_vault.json");
        std::fs::write(&path, b"{broken").unwrap();
        assert!(read_vault(&path).is_err());
        assert!(ensure_key(&dir.path().join(".psk_vault.key")).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{broken");
    }

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
        let _serial = STORE_TEST_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
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
