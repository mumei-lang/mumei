//! Detached producer-signature envelope (mumei-agent issue #886 design memo).
//!
//! `sign` turns any bounded record's raw bytes plus a caller-declared ed25519
//! secret key into one `mumei.producer-signature/v1` envelope — the detached
//! authenticity record that travels beside the record, never inside it, so
//! every pin on the record stays byte-identical. `subject_digest` is the
//! record's SHA-256 (lowercase hex, matching `evidence_fingerprint` on the
//! acceptance descriptor); the signed payload is the digest's ASCII bytes,
//! matching the `crypto_verify_signature` FFI convention.
//!
//! A valid envelope only binds the declared key to the declared digest: it
//! says nothing about the key's own authenticity, freshness or revocation,
//! and signing grants no acceptance, grade or truth claim.

use ed25519_dalek::{Signer, SigningKey};
use serde::Serialize;
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

pub const PRODUCER_SIGNATURE_SCHEMA: &str = "mumei.producer-signature/v1";

/// Closed envelope shape; `producer_key_id` is a caller-declared label the
/// consumer pins separately — it is never derived from the key bytes.
#[derive(Debug, Serialize)]
pub struct Envelope {
    pub schema_version: &'static str,
    pub subject_digest: String,
    pub algorithm: &'static str,
    pub signature: String,
    pub producer_key_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignError {
    /// The secret-key material is not 64-char lowercase hex of 32 bytes.
    InvalidKeyMaterial,
    /// The declared producer key id is empty.
    InvalidProducerKeyId,
}

/// Decode a 32-byte ed25519 secret from its 64-char lowercase-hex form —
/// the same shape `signature-verify` expects for the public key. Strictly
/// ASCII: no whitespace, prefix or surrounding bytes.
pub fn decode_secret_key(text: &str) -> Result<SigningKey, SignError> {
    let bytes = text.as_bytes();
    let mut raw = [0u8; 32];
    if bytes.len() != 64 {
        return Err(SignError::InvalidKeyMaterial);
    }
    for (index, byte) in bytes.iter().enumerate() {
        let nibble = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => return Err(SignError::InvalidKeyMaterial),
        };
        raw[index / 2] |= nibble << (4 * (1 - index % 2));
    }
    let key = SigningKey::from_bytes(&raw);
    raw.zeroize();
    Ok(key)
}

/// Decode a secret key from owned file bytes and zeroize the buffer on
/// every path, so key material does not linger in caller memory.
pub fn decode_secret_key_bytes(mut bytes: Vec<u8>) -> Result<SigningKey, SignError> {
    let result = std::str::from_utf8(&bytes)
        .map_err(|_| SignError::InvalidKeyMaterial)
        .and_then(decode_secret_key);
    bytes.zeroize();
    result
}

/// SHA-256 over the record's raw bytes — the consumer reproduces it with
/// `sha256sum` over the same file, so no canonicalization is needed.
pub fn subject_digest(record: &[u8]) -> String {
    format!("{:x}", Sha256::digest(record))
}

/// Sign `record` under `secret_key` and emit the detached envelope.
/// The signature covers the digest's lowercase-hex ASCII bytes.
pub fn sign(
    record: &[u8],
    secret_key: &SigningKey,
    producer_key_id: &str,
) -> Result<Envelope, SignError> {
    if producer_key_id.is_empty() {
        return Err(SignError::InvalidProducerKeyId);
    }
    let digest = subject_digest(record);
    let signature = secret_key.sign(digest.as_bytes());
    Ok(Envelope {
        schema_version: PRODUCER_SIGNATURE_SCHEMA,
        subject_digest: digest,
        algorithm: "ed25519",
        signature: hex(&signature.to_bytes()),
        producer_key_id: producer_key_id.to_string(),
    })
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((byte & 0xf) as u32, 16).unwrap());
    }
    out
}

#[cfg(test)]
mod tests;
