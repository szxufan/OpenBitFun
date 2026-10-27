//! Device-pair message keys for GitHub-authenticated OpenBitFun Relay sessions.
//! Public keys come from the relay's same-account directory. Private keys are
//! generated and retained by each device. The relay wire envelope is unchanged.

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

pub const KDF_SALT: &[u8] = b"OpenBitFun Relay v1.0.0 device key";

/// Domain-separated salt for the PSK-paired key derivation. Never reuse the
/// legacy salt: an unpaired derivation must not collide with a paired one.
pub const PSK_KDF_SALT: &[u8] = b"OpenBitFun Relay v1.1.0 device key psk";

/// Wire prefix carried in the encrypted envelope's optional `key_id` field.
pub const PSK_KEY_ID_PREFIX: &str = "psk_v1:";

pub fn generate_secret() -> [u8; 32] {
    StaticSecret::random_from_rng(rand::rngs::OsRng).to_bytes()
}

/// Derive a distinct bootstrap key that is stable for an idempotent request.
/// Knowledge of the target key does not reveal the provisioning device's key.
pub fn provisioning_secret(
    parent_secret: &[u8; 32],
    device_id: &str,
    request_id: &str,
) -> [u8; 32] {
    let mut secret = [0; 32];
    let info = format!("{device_id}:{request_id}");
    Hkdf::<Sha256>::new(
        Some(b"OpenBitFun device provisioning v1.0.0"),
        parent_secret,
    )
    .expand(info.as_bytes(), &mut secret)
    .expect("32-byte HKDF output is valid");
    secret
}

pub fn public_key(secret: &[u8; 32]) -> [u8; 32] {
    PublicKey::from(&StaticSecret::from(*secret)).to_bytes()
}

pub fn public_key_base64(secret: &[u8; 32]) -> String {
    BASE64.encode(public_key(secret))
}

/// X25519 + HKDF-SHA256. Ordering public keys makes the derivation symmetric,
/// while binding both identities and the protocol domain to the final AES key.
pub fn derive_message_key(secret: &[u8; 32], peer_public: &[u8; 32]) -> Result<[u8; 32]> {
    let local_public = public_key(secret);
    let shared = StaticSecret::from(*secret).diffie_hellman(&PublicKey::from(*peer_public));
    if !shared.was_contributory() {
        return Err(anyhow!("invalid peer public key"));
    }
    let (first, second) = if local_public < *peer_public {
        (&local_public, peer_public)
    } else {
        (peer_public, &local_public)
    };
    let mut info = [0u8; 64];
    info[..32].copy_from_slice(first);
    info[32..].copy_from_slice(second);
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(Some(KDF_SALT), shared.as_bytes())
        .expand(&info, &mut key)
        .map_err(|_| anyhow!("device message key derivation failed"))?;
    Ok(key)
}

/// Short stable identifier for an out-of-band pairing secret. Safe to display
/// and compare across devices; reveals nothing about the secret itself.
pub fn psk_id(psk: &[u8; 32]) -> [u8; 8] {
    let digest = Sha256::new()
        .chain_update(b"OpenBitFun PSK id v1")
        .chain_update(psk)
        .finalize();
    let mut id = [0u8; 8];
    id.copy_from_slice(&digest[..8]);
    id
}

/// The `key_id` wire value a PSK-paired sender attaches to its envelope.
pub fn psk_key_id(psk: &[u8; 32]) -> String {
    let id = psk_id(psk);
    id.iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

/// PSK-paired variant of [`derive_message_key`]. The out-of-band pairing
/// secret is concatenated with the X25519 shared secret as HKDF input keying
/// material, so a relay that swaps directory public keys cannot derive the
/// message key even though it relays every envelope. The PSK id enters the
/// HKDF info so rotating the pairing secret rotates the derived keys.
pub fn derive_message_key_psk(
    secret: &[u8; 32],
    peer_public: &[u8; 32],
    psk: &[u8; 32],
) -> Result<[u8; 32]> {
    let local_public = public_key(secret);
    let shared = StaticSecret::from(*secret).diffie_hellman(&PublicKey::from(*peer_public));
    if !shared.was_contributory() {
        return Err(anyhow!("invalid peer public key"));
    }
    let (first, second) = if local_public < *peer_public {
        (&local_public, peer_public)
    } else {
        (peer_public, &local_public)
    };
    let id = psk_id(psk);
    let mut info = [0u8; 72];
    info[..32].copy_from_slice(first);
    info[32..64].copy_from_slice(second);
    info[64..].copy_from_slice(&id);
    let mut ikm = [0u8; 64];
    ikm[..32].copy_from_slice(shared.as_bytes());
    ikm[32..].copy_from_slice(psk);
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(Some(PSK_KDF_SALT), &ikm)
        .expand(&info, &mut key)
        .map_err(|_| anyhow!("psk device message key derivation failed"))?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_connect::encryption::{decrypt_from_base64, encrypt_to_base64};

    #[test]
    fn provisioning_keys_are_isolated_and_replayable() {
        let parent = [7; 32];
        let key = provisioning_secret(&parent, "target", "request");
        assert_ne!(key, parent);
        assert_eq!(key, provisioning_secret(&parent, "target", "request"));
        assert_ne!(key, provisioning_secret(&parent, "other", "request"));
        assert_ne!(key, provisioning_secret(&parent, "target", "other"));
    }

    #[test]
    fn messages_round_trip_between_independent_devices_only() {
        let alice = [7; 32];
        let bob = [11; 32];
        let charlie = [17; 32];
        let sending = derive_message_key(&alice, &public_key(&bob)).unwrap();
        let receiving = derive_message_key(&bob, &public_key(&alice)).unwrap();
        assert_eq!(sending, receiving);
        assert_eq!(
            sending
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            "6e8f5da837e91e9ddb09c5aa7dee229e731fc94499d29d10dcf5f1437193f56c"
        );
        let (data, nonce) = encrypt_to_base64(&sending, "remote approval").unwrap();
        assert_eq!(
            decrypt_from_base64(&receiving, &data, &nonce).unwrap(),
            "remote approval"
        );
        let unrelated = derive_message_key(&charlie, &public_key(&alice)).unwrap();
        assert!(decrypt_from_base64(&unrelated, &data, &nonce).is_err());
        assert!(derive_message_key(&alice, &[0; 32]).is_err());
        let mut low_order = [0; 32];
        low_order[0] = 1;
        assert!(derive_message_key(&alice, &low_order).is_err());
    }

    #[test]
    fn psk_keys_are_symmetric_distinct_and_pinned() {
        let alice = [7; 32];
        let bob = [11; 32];
        let psk = [91; 32];
        let sending = derive_message_key_psk(&alice, &public_key(&bob), &psk).unwrap();
        let receiving = derive_message_key_psk(&bob, &public_key(&alice), &psk).unwrap();
        assert_eq!(sending, receiving);
        assert_eq!(
            sending
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            "f55f939ca726bfae284fb0263acbadbd8b010f4ff5f7d47663f0a0706149c827"
        );
        // The paired derivation must never collide with the legacy one, with a
        // different pairing secret, or with the same secret under a peer swap.
        assert_ne!(sending, derive_message_key(&alice, &public_key(&bob)).unwrap());
        let other_psk = [92; 32];
        assert_ne!(sending, derive_message_key_psk(&alice, &public_key(&bob), &other_psk).unwrap());
        assert_ne!(psk_id(&psk), psk_id(&other_psk));
        assert_eq!(psk_key_id(&psk).len(), 16);
        // A swapped (MITM) public key cannot reproduce the paired key.
        let mallory = [33; 32];
        let via_relay = derive_message_key_psk(&alice, &public_key(&mallory), &psk).unwrap();
        let bob_side = derive_message_key_psk(&bob, &public_key(&alice), &psk).unwrap();
        assert_ne!(via_relay, bob_side);
        let (data, nonce) = encrypt_to_base64(&sending, "paired traffic").unwrap();
        assert!(decrypt_from_base64(&via_relay, &data, &nonce).is_err());
        assert_eq!(
            decrypt_from_base64(&receiving, &data, &nonce).unwrap(),
            "paired traffic"
        );
        let mut low_order = [0; 32];
        low_order[0] = 1;
        assert!(derive_message_key_psk(&alice, &low_order, &psk).is_err());
    }
}
