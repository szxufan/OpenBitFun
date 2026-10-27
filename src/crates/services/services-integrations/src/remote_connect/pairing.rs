//! Invitation status and account-linked IM enrollment codes.
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::device_crypto;

/// Current state of the pairing process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingState {
    Idle,
    WaitingForScan,
    Handshaking,
    Verifying,
    Connected,
    Failed { reason: String },
    Disconnected,
}

pub fn generate_bot_pairing_code() -> String {
    let code: u32 = rand::thread_rng().gen_range(100_000..1_000_000);
    format!("{code:06}")
}

// ── Out-of-band PSK pairing codes ────────────────────────────────────────
//
// A pairing code carries the 256-bit pairing secret, the offerer's device id
// and X25519 public key, and a 4-byte check value, Crockford-Base32 encoded.
// It travels over a channel the user chooses (QR photo, copied text). The
// completer compares the embedded public key with the relay directory's value;
// a mismatch means the directory may be under a man-in-the-middle attack.

const PAIRING_CODE_VERSION_1: u8 = 1;
const CHECK_LEN: usize = 4;
const DEVICE_ID_BYTES: usize = 16;
/// 1 version + 32 psk + 16 device id + 32 public key + 4 check.
const PAYLOAD_LEN: usize = 1 + 32 + DEVICE_ID_BYTES + 32 + CHECK_LEN;

/// Crockford Base32 without I, L, O, U.
const CROCKFORD_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// What a parsed pairing code carries. The PSK id is derived, not transported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PskPairingPayload {
    pub psk: [u8; 32],
    pub offerer_device_id: String,
    pub offerer_public_key: [u8; 32],
}

impl PskPairingPayload {
    pub fn psk_id(&self) -> String {
        device_crypto::psk_key_id(&self.psk)
    }
}

/// Generate a fresh random pairing secret.
pub fn generate_psk() -> [u8; 32] {
    device_crypto::generate_secret()
}

fn check_value(payload_prefix: &[u8]) -> [u8; CHECK_LEN] {
    let digest = Sha256::new()
        .chain_update(b"OpenBitFun PSK pairing code v1")
        .chain_update(payload_prefix)
        .finalize();
    let mut check = [0u8; CHECK_LEN];
    check.copy_from_slice(&digest[..CHECK_LEN]);
    check
}

fn base32_encode(data: &[u8]) -> String {
    debug_assert_eq!(data.len() % 5, 0, "payload length must pack into 5-byte groups");
    let mut out = String::with_capacity(data.len() / 5 * 8);
    for chunk in data.chunks(5) {
        let mut buf = 0u64;
        for byte in chunk {
            buf = (buf << 8) | *byte as u64;
        }
        // 5 bytes = 40 bits = 8 five-bit groups, most significant first.
        for shift in (0..8).rev() {
            let index = ((buf >> (shift * 5)) & 0x1f) as usize;
            out.push(CROCKFORD_ALPHABET[index] as char);
        }
    }
    out
}

fn base32_decode(text: &str) -> Result<Vec<u8>, anyhow::Error> {
    let mut bits: Vec<u8> = Vec::with_capacity(text.len() * 5);
    for ch in text.chars() {
        let value = match ch {
            '0' | 'O' | 'o' => 0,
            '1' | 'I' | 'i' | 'L' | 'l' => 1,
            '2'..='9' => ch as u8 - b'0',
            'A'..='H' | 'J'..='K' | 'M'..='N' | 'P'..='T' | 'V'..='Z' => {
                let upper = ch.to_ascii_uppercase();
                let index = CROCKFORD_ALPHABET
                    .iter()
                    .position(|candidate| *candidate == upper as u8)
                    .expect("character class checked above");
                index as u8
            }
            _ => anyhow::bail!("invalid pairing code character: {ch:?}"),
        };
        for shift in (0..5).rev() {
            bits.push((value >> shift) & 1);
        }
    }
    let mut out = Vec::with_capacity(bits.len() / 8);
    for byte_bits in bits.chunks(8) {
        if byte_bits.len() != 8 {
            anyhow::bail!("pairing code length is not a multiple of 8 characters");
        }
        let mut byte = 0u8;
        for bit in byte_bits {
            byte = (byte << 1) | bit;
        }
        out.push(byte);
    }
    Ok(out)
}

/// Encode a pairing payload as grouped Crockford-Base32 text (4 characters
/// per group, `-` separated) for display, copy, and manual entry.
pub fn encode_psk_pairing_code(payload: &PskPairingPayload) -> anyhow::Result<String> {
    let device_id_bytes = hex::decode(payload.offerer_device_id.trim())
        .map_err(|e| anyhow::anyhow!("device id must be hex: {e}"))?;
    if device_id_bytes.len() != DEVICE_ID_BYTES {
        anyhow::bail!("device id must encode to {DEVICE_ID_BYTES} bytes");
    }
    let mut body = Vec::with_capacity(PAYLOAD_LEN - CHECK_LEN);
    body.push(PAIRING_CODE_VERSION_1);
    body.extend_from_slice(&payload.psk);
    body.extend_from_slice(&device_id_bytes);
    body.extend_from_slice(&payload.offerer_public_key);
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

/// Parse and validate a pairing code (dashes, spaces, and lowercase are
/// tolerated; `O`/`I`/`L` map to `0`/`1` as Crockford specifies).
pub fn parse_psk_pairing_code(code: &str) -> anyhow::Result<PskPairingPayload> {
    let compact: String = code
        .chars()
        .filter(|ch| !matches!(ch, '-' | ' ' | '\t' | '\r' | '\n'))
        .collect();
    let data = base32_decode(&compact)?;
    if data.len() != PAYLOAD_LEN {
        anyhow::bail!("unexpected pairing code length");
    }
    let (body, check) = data.split_at(PAYLOAD_LEN - CHECK_LEN);
    if body[0] != PAIRING_CODE_VERSION_1 {
        anyhow::bail!("unsupported pairing code version: {}", body[0]);
    }
    if check_value(body).as_slice() != check {
        anyhow::bail!("pairing code check failed; please re-copy the code");
    }
    let mut psk = [0u8; 32];
    psk.copy_from_slice(&body[1..33]);
    let mut device_id = [0u8; DEVICE_ID_BYTES];
    device_id.copy_from_slice(&body[33..33 + DEVICE_ID_BYTES]);
    let mut public_key = [0u8; 32];
    public_key.copy_from_slice(&body[33 + DEVICE_ID_BYTES..]);
    Ok(PskPairingPayload {
        psk,
        offerer_device_id: hex::encode(device_id),
        offerer_public_key: public_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_payload() -> PskPairingPayload {
        PskPairingPayload {
            psk: [91u8; 32],
            offerer_device_id: "0123456789abcdef0123456789abcdef".to_string(),
            offerer_public_key: device_crypto::public_key(&[7u8; 32]),
        }
    }

    #[test]
    fn pairing_codes_round_trip_and_reject_tampering() {
        let payload = sample_payload();
        let code = encode_psk_pairing_code(&payload).unwrap();
        // Pinned vector shared with the TypeScript mirror
        // (`src/mobile-web/tests/account-login.test.mjs`).
        assert_eq!(
            code,
            "05DN-PPTV-BDDN-PPTV-BDDN-PPTV-BDDN-PPTV-BDDN-PPTV-BDDN-PPTV-BDDN-P093-8NKR-KAYD-XW0J-6HB7-H6NW-VVRK-QS7Y-NTQJ-0K3Z-TCTR-ZJE0-0WGR-G78Q-89W1-50H7-XHKM-YDZQ-ZTBV-DQDQ-TCAQ"
        );
        // Grouped display format, decodable with separators and case noise.
        assert!(code.contains('-'));
        let parsed = parse_psk_pairing_code(&code).unwrap();
        assert_eq!(parsed, payload);
        assert_eq!(parsed.psk_id(), device_crypto::psk_key_id(&payload.psk));

        let noisy = code.to_lowercase().replace('-', " ");
        assert_eq!(parse_psk_pairing_code(&noisy).unwrap(), payload);

        // A single flipped character must fail the check or the alphabet.
        let mut flipped: Vec<char> = code.chars().collect();
        let index = flipped
            .iter()
            .position(|ch| *ch != '-')
            .expect("code has data characters");
        flipped[index] = if flipped[index] == 'A' { 'B' } else { 'A' };
        assert!(parse_psk_pairing_code(&flipped.into_iter().collect::<String>()).is_err());

        // Wrong device id shape and truncation are rejected.
        let mut bad_id = sample_payload();
        bad_id.offerer_device_id = "not-hex".to_string();
        assert!(encode_psk_pairing_code(&bad_id).is_err());
        assert!(parse_psk_pairing_code(&code[..code.len() - 6]).is_err());
    }
}
