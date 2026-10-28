//! Manual, one-time-code-encrypted config backups stored on the relay.
//!
//! A backup code is a fresh random 256-bit secret shown to the user once at
//! export time (Crockford-Base32 text, same codec family as the PSK pairing
//! code but a separate domain, so one format can never be mistaken for the
//! other). The AES-256-GCM key is HKDF-derived from that secret; the relay
//! stores only ciphertext, a nonce, and a non-secret key hint. Import is
//! manual: the user pastes the code, and nothing syncs automatically.

use anyhow::{anyhow, bail, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

use super::encryption;
use super::pairing::{base32_decode, base32_encode};

pub const CONFIG_BACKUP_SCHEME: &str = "backup_code_v1";

/// Domain-separated HKDF salt. Never reuse the device/PSK message-key salts:
/// a config backup key must not collide with any peer-envelope derivation.
const CONFIG_BACKUP_KDF_SALT: &[u8] = b"OpenBitFun config backup v1";
const CONFIG_BACKUP_KDF_INFO: &[u8] = b"config_backup_code";

/// Non-secret prefix plus 8-byte key digest shown in relay metadata so a
/// wrong code can be diagnosed before a GCM failure, without leaking the key.
const KEY_HINT_PREFIX: &str = "code_v1:";

const CODE_VERSION_1: u8 = 1;
const CHECK_LEN: usize = 4;
/// 1 version + 32 secret + 3 reserved zero bytes + 4 check = 40 bytes, so the
/// payload packs into 5-byte base32 groups (64 characters, 16 dashed groups).
const CODE_PAYLOAD_LEN: usize = 1 + 32 + 3 + CHECK_LEN;

fn check_value(payload_prefix: &[u8]) -> [u8; CHECK_LEN] {
    let digest = Sha256::new()
        .chain_update(b"OpenBitFun config backup code v1")
        .chain_update(payload_prefix)
        .finalize();
    let mut check = [0u8; CHECK_LEN];
    check.copy_from_slice(&digest[..CHECK_LEN]);
    check
}

/// Generate a fresh backup secret and encode it as grouped display text.
pub fn generate_config_backup_code() -> String {
    let secret = super::device_crypto::generate_secret();
    encode_config_backup_code(&secret).expect("fresh secret encodes")
}

fn encode_config_backup_code(secret: &[u8; 32]) -> Result<String> {
    let mut body = Vec::with_capacity(CODE_PAYLOAD_LEN - CHECK_LEN);
    body.push(CODE_VERSION_1);
    body.extend_from_slice(secret);
    body.extend_from_slice(&[0u8; 3]);
    let check = check_value(&body);
    body.extend_from_slice(&check);
    let encoded = base32_encode(&body);
    Ok(encoded
        .as_bytes()
        .chunks(4)
        .map(std::str::from_utf8)
        .collect::<Result<Vec<_>, _>>()?
        .join("-"))
}

/// Parse and validate a backup code (dashes, spaces, and lowercase tolerated;
/// `O`/`I`/`L` map to `0`/`1` as Crockford specifies). Returns the 256-bit
/// secret.
pub fn parse_config_backup_code(code: &str) -> Result<[u8; 32]> {
    let compact: String = code
        .chars()
        .filter(|ch| !matches!(ch, '-' | ' ' | '\t' | '\r' | '\n'))
        .collect();
    let data = base32_decode(&compact)?;
    if data.len() != CODE_PAYLOAD_LEN {
        bail!("unexpected config backup code length");
    }
    let (body, check) = data.split_at(CODE_PAYLOAD_LEN - CHECK_LEN);
    if body[0] != CODE_VERSION_1 {
        bail!("unsupported config backup code version: {}", body[0]);
    }
    if body[33..36] != [0u8; 3] {
        bail!("malformed config backup code padding");
    }
    if check_value(body).as_slice() != check {
        bail!("config backup code check failed; please re-copy the code");
    }
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&body[1..33]);
    Ok(secret)
}

/// HKDF-SHA256 from the backup secret to the AES-256-GCM key.
pub fn derive_config_backup_key(secret: &[u8; 32]) -> [u8; 32] {
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(Some(CONFIG_BACKUP_KDF_SALT), secret)
        .expand(CONFIG_BACKUP_KDF_INFO, &mut key)
        .expect("32-byte HKDF output is valid");
    key
}

/// Non-secret identifier for the derived key, safe to store on the relay.
pub fn config_backup_key_hint(key: &[u8; 32]) -> String {
    let digest = Sha256::new()
        .chain_update(b"OpenBitFun config backup key id v1")
        .chain_update(key)
        .finalize();
    format!("{KEY_HINT_PREFIX}{}", hex::encode(&digest[..8]))
}

pub fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
        .max(1)
}

/// Upload body for `PUT /api/account/config-backup/blob`.
#[derive(Serialize)]
pub struct ConfigBackupUpload {
    pub scheme: String,
    pub key_hint: String,
    pub encrypted_data: String,
    pub nonce: String,
    pub version: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigBackupMeta {
    pub scheme: String,
    pub key_hint: String,
    pub size_bytes: i64,
    pub version: i64,
    pub updated_at: i64,
}

/// Full stored envelope returned by `GET /api/account/config-backup/blob`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigBackupEnvelope {
    pub scheme: String,
    pub key_hint: String,
    pub encrypted_data: String,
    pub nonce: String,
    pub version: i64,
    pub updated_at: i64,
}

/// Encrypt a config export document for relay storage. The caller keeps the
/// backup code; nothing derived from it travels in the upload.
pub fn seal_config_backup(
    secret: &[u8; 32],
    plaintext: &str,
    version: i64,
) -> Result<ConfigBackupUpload> {
    if version <= 0 {
        bail!("config backup version must be positive");
    }
    let key = derive_config_backup_key(secret);
    let (ciphertext, nonce) = encryption::encrypt(&key, plaintext.as_bytes())?;
    Ok(ConfigBackupUpload {
        scheme: CONFIG_BACKUP_SCHEME.to_string(),
        key_hint: config_backup_key_hint(&key),
        encrypted_data: BASE64.encode(ciphertext),
        nonce: BASE64.encode(nonce),
        version,
    })
}

/// Decrypt a downloaded envelope with the user-supplied backup code. Failures
/// are explicit: a hint mismatch or GCM failure means the code is wrong or
/// the backup is damaged, never a silent empty result.
pub fn open_config_backup(secret: &[u8; 32], envelope: &ConfigBackupEnvelope) -> Result<String> {
    if envelope.scheme != CONFIG_BACKUP_SCHEME {
        bail!("unsupported config backup scheme: {}", envelope.scheme);
    }
    let key = derive_config_backup_key(secret);
    let hint = config_backup_key_hint(&key);
    if hint != envelope.key_hint {
        bail!("backup code does not match the stored backup");
    }
    let ciphertext = BASE64
        .decode(&envelope.encrypted_data)
        .map_err(|e| anyhow!("decode config backup ciphertext: {e}"))?;
    let nonce_bytes = BASE64
        .decode(&envelope.nonce)
        .map_err(|e| anyhow!("decode config backup nonce: {e}"))?;
    let nonce: [u8; 12] = nonce_bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("config backup nonce must be 12 bytes"))?;
    let plaintext = encryption::decrypt(&key, &ciphertext, &nonce)
        .map_err(|_| anyhow!("backup code is incorrect or the backup is damaged"))?;
    String::from_utf8(plaintext).map_err(|e| anyhow!("config backup is not valid UTF-8: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_codes_round_trip_and_survive_retyping() {
        let code = generate_config_backup_code();
        assert_eq!(code.split('-').count(), 16);
        assert!(code
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '-'));

        let secret = parse_config_backup_code(&code).unwrap();
        // Retyped with spaces, lowercase, and Crockford confusables.
        let noisy = code
            .to_lowercase()
            .replace('-', "  ")
            .replace('o', "0")
            .replace('i', "1");
        assert_eq!(parse_config_backup_code(&noisy).unwrap(), secret);

        // A flipped character must fail the check or the alphabet.
        let mut flipped: Vec<char> = code.chars().collect();
        let index = flipped.iter().position(|ch| *ch != '-').unwrap();
        flipped[index] = if flipped[index] == 'A' { 'B' } else { 'A' };
        assert!(parse_config_backup_code(&flipped.into_iter().collect::<String>()).is_err());

        // Truncation is rejected.
        assert!(parse_config_backup_code(&code[..code.len() - 5]).is_err());
    }

    #[test]
    fn pinned_code_vector_is_stable() {
        let code = encode_config_backup_code(&[77u8; 32]).unwrap();
        // Pinned so a codec change cannot silently invalidate saved codes.
        assert_eq!(
            code,
            "056M-TKAD-9N6M-TKAD-9N6M-TKAD-9N6M-TKAD-9N6M-TKAD-9N6M-TKAD-9N6M-T000-0120-55FS"
        );
    }

    #[test]
    fn sealed_backups_round_trip_and_reject_wrong_codes() {
        let secret = parse_config_backup_code(&generate_config_backup_code()).unwrap();
        let plaintext = r#"{"product_id":"openbitfun","config":{}}"#;
        let upload = seal_config_backup(&secret, plaintext, unix_millis()).unwrap();
        assert_eq!(upload.scheme, CONFIG_BACKUP_SCHEME);
        assert!(upload.key_hint.starts_with(KEY_HINT_PREFIX));

        let envelope = ConfigBackupEnvelope {
            scheme: upload.scheme.clone(),
            key_hint: upload.key_hint.clone(),
            encrypted_data: upload.encrypted_data.clone(),
            nonce: upload.nonce.clone(),
            version: upload.version,
            updated_at: upload.version,
        };
        assert_eq!(open_config_backup(&secret, &envelope).unwrap(), plaintext);

        let wrong = parse_config_backup_code(&generate_config_backup_code()).unwrap();
        let error = open_config_backup(&wrong, &envelope)
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not match"), "{error}");

        // Tampered ciphertext fails even with the right code.
        let mut tampered = envelope.clone();
        tampered.encrypted_data.replace_range(4..8, "AAAA");
        assert!(open_config_backup(&secret, &tampered).is_err());

        // Unknown scheme is loud, not a best-effort guess.
        let mut future = envelope.clone();
        future.scheme = "backup_code_v2".to_string();
        assert!(open_config_backup(&secret, &future)
            .unwrap_err()
            .to_string()
            .contains("unsupported"));
    }

    #[test]
    fn hint_identifies_the_key_without_leaking_it() {
        let a = derive_config_backup_key(&[1u8; 32]);
        let b = derive_config_backup_key(&[2u8; 32]);
        assert_ne!(config_backup_key_hint(&a), config_backup_key_hint(&b));
        assert_eq!(config_backup_key_hint(&a).len(), KEY_HINT_PREFIX.len() + 16);
    }
}
