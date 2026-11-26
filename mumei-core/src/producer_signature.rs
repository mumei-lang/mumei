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

use crate::verification_evidence;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

use zeroize::Zeroize;

pub const PRODUCER_SIGNATURE_SCHEMA: &str = "mumei.producer-signature/v1";
pub const PRODUCER_KEY_REGISTRY_SCHEMA: &str = "mumei.producer-key-registry/v1";
pub const KEY_REVOCATION_LIST_SCHEMA: &str = "mumei.key-revocation-list/v1";
pub const KEY_REVOCATION_LIST_V2_SCHEMA: &str = "mumei.key-revocation-list/v2";

/// Maximum raw subject size accepted by the sign command (4 MiB).
pub const MAX_SUBJECT_BYTES: u64 = 4 * 1024 * 1024;

/// Closed envelope shape; `producer_key_id` is a caller-declared label the
/// consumer pins separately — it is never derived from the key bytes.
/// `produced_at` joins the envelope only when the caller stamps one; like
/// the key id it is declared, never signed — the signature covers the
/// digest alone.
#[derive(Debug, Serialize)]
pub struct Envelope {
    pub schema_version: &'static str,
    pub subject_digest: String,
    pub algorithm: &'static str,
    pub signature: String,
    pub producer_key_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub produced_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignError {
    /// The secret-key material is not 64-char lowercase hex of 32 bytes.
    InvalidKeyMaterial,
    /// The declared producer key id is empty.
    InvalidProducerKeyId,
    /// The declared `produced_at` is not an RFC 3339 timestamp.
    InvalidProducedAt,
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

/// Generate a fresh ed25519 producer keypair from OS entropy and return the
/// 64-char lowercase-hex encodings `(secret, public)` — the same shapes
/// `--key-file` and `signature-verify`'s producer key take. The caller owns
/// persisting the secret; it must never be printed.
pub fn generate_keypair() -> (String, String) {
    let mut secret = ed25519_dalek::SecretKey::default();
    getrandom::fill(&mut secret).expect("OS entropy");
    let key = SigningKey::from_bytes(&secret);
    secret.zeroize();
    let mut bytes = key.to_bytes();
    let secret_hex = hex(&bytes);
    bytes.zeroize();
    (secret_hex, hex(key.verifying_key().as_bytes()))
}

/// SHA-256 over the record's raw bytes — the consumer reproduces it with
/// `sha256sum` over the same file, so no canonicalization is needed.
pub fn subject_digest(record: &[u8]) -> String {
    format!("{:x}", Sha256::digest(record))
}

pub fn envelope_match(envelope: &Value) -> bool {
    let Some(object) = envelope.as_object() else {
        return false;
    };
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "schema_version"
                | "subject_digest"
                | "algorithm"
                | "signature"
                | "producer_key_id"
                | "produced_at"
        )
    }) {
        return false;
    }
    let required = [
        "schema_version",
        "subject_digest",
        "algorithm",
        "signature",
        "producer_key_id",
    ];
    if required.iter().any(|key| !object.contains_key(*key)) {
        return false;
    }
    object.get("schema_version").and_then(Value::as_str) == Some(PRODUCER_SIGNATURE_SCHEMA)
        && object
            .get("subject_digest")
            .and_then(Value::as_str)
            .is_some_and(|value| is_lower_hex(value, 64))
        && object.get("algorithm").and_then(Value::as_str) == Some("ed25519")
        && object
            .get("signature")
            .and_then(Value::as_str)
            .is_some_and(|value| is_lower_hex(value, 128))
        && object
            .get("producer_key_id")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        && object.get("produced_at").is_none_or(Value::is_string)
}

pub fn decode_public_key_hex(value: &str) -> Option<[u8; 32]> {
    decode_lower_hex(value)
}

pub fn signature_valid(public_key_hex: &str, signature_hex: &str, message: &[u8]) -> bool {
    let Some(public_key_bytes) = decode_public_key_hex(public_key_hex) else {
        return false;
    };
    let Some(signature_bytes) = decode_lower_hex::<64>(signature_hex) else {
        return false;
    };
    let Ok(public_key) = VerifyingKey::from_bytes(&public_key_bytes) else {
        return false;
    };
    let signature = Signature::from_bytes(&signature_bytes);
    public_key.verify(message, &signature).is_ok()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryEntry {
    pub principal: String,
    pub not_before: (i128, i128),
    pub not_after: (i128, i128),
}

pub type ProducerKeyRegistry = BTreeMap<String, RegistryEntry>;
pub type RevocationKeySet = BTreeSet<String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RevocationMetadata {
    pub list_version: u64,
    pub issued_at: (i128, i128),
    pub not_after: Option<(i128, i128)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevocationList {
    pub revoked_keys: RevocationKeySet,
    pub metadata: Option<RevocationMetadata>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorError {
    InvalidDocument,
}

pub fn parse_key_registry(bytes: &[u8]) -> Result<ProducerKeyRegistry, AnchorError> {
    use crate::strict_json::{self, JsonValue};

    let value = strict_json::parse(bytes).map_err(|_| AnchorError::InvalidDocument)?;
    let object = strict_json::object(&value).map_err(|_| AnchorError::InvalidDocument)?;
    strict_json::require_keys(object, &["schema_version", "registry"])
        .map_err(|_| AnchorError::InvalidDocument)?;
    if strict_json::string_field(object, "schema_version")
        .map_err(|_| AnchorError::InvalidDocument)?
        != PRODUCER_KEY_REGISTRY_SCHEMA
    {
        return Err(AnchorError::InvalidDocument);
    }
    let registry =
        match strict_json::field(object, "registry").map_err(|_| AnchorError::InvalidDocument)? {
            JsonValue::Object(registry) if !registry.is_empty() => registry,
            _ => return Err(AnchorError::InvalidDocument),
        };
    let mut parsed = ProducerKeyRegistry::new();
    for (key_id, entry) in registry {
        if key_id.is_empty() {
            return Err(AnchorError::InvalidDocument);
        }
        let entry = strict_json::object(entry).map_err(|_| AnchorError::InvalidDocument)?;
        strict_json::require_keys(entry, &["principal", "not_before", "not_after"])
            .map_err(|_| AnchorError::InvalidDocument)?;
        let principal = strict_json::string_field(entry, "principal")
            .map_err(|_| AnchorError::InvalidDocument)?;
        if principal.is_empty() {
            return Err(AnchorError::InvalidDocument);
        }
        let not_before = verification_evidence::rfc3339_instant(
            strict_json::string_field(entry, "not_before")
                .map_err(|_| AnchorError::InvalidDocument)?,
        )
        .ok_or(AnchorError::InvalidDocument)?;
        let not_after = verification_evidence::rfc3339_instant(
            strict_json::string_field(entry, "not_after")
                .map_err(|_| AnchorError::InvalidDocument)?,
        )
        .ok_or(AnchorError::InvalidDocument)?;
        if not_before.0 > not_after.1 {
            return Err(AnchorError::InvalidDocument);
        }
        parsed.insert(
            key_id.clone(),
            RegistryEntry {
                principal: principal.to_owned(),
                not_before,
                not_after,
            },
        );
    }
    Ok(parsed)
}

pub fn parse_revocation_list(bytes: &[u8]) -> Result<RevocationList, AnchorError> {
    use crate::strict_json::{self, JsonValue};

    let value = strict_json::parse(bytes).map_err(|_| AnchorError::InvalidDocument)?;
    let object = strict_json::object(&value).map_err(|_| AnchorError::InvalidDocument)?;
    let schema_version = strict_json::string_field(object, "schema_version")
        .map_err(|_| AnchorError::InvalidDocument)?;
    let metadata = match schema_version {
        KEY_REVOCATION_LIST_SCHEMA => {
            strict_json::require_keys(object, &["schema_version", "revoked_keys"])
                .map_err(|_| AnchorError::InvalidDocument)?;
            None
        }
        KEY_REVOCATION_LIST_V2_SCHEMA => {
            let required = [
                "schema_version",
                "list_version",
                "issued_at",
                "revoked_keys",
            ];
            if required.iter().any(|field| !object.contains_key(*field))
                || object
                    .keys()
                    .any(|field| !required.contains(&field.as_str()) && field != "not_after")
            {
                return Err(AnchorError::InvalidDocument);
            }
            let list_version = strict_json::unsigned_field(object, "list_version")
                .map_err(|_| AnchorError::InvalidDocument)?;
            if list_version == 0 {
                return Err(AnchorError::InvalidDocument);
            }
            let issued_at = verification_evidence::rfc3339_instant(
                strict_json::string_field(object, "issued_at")
                    .map_err(|_| AnchorError::InvalidDocument)?,
            )
            .ok_or(AnchorError::InvalidDocument)?;
            let not_after = object
                .contains_key("not_after")
                .then(|| {
                    strict_json::string_field(object, "not_after")
                        .map_err(|_| AnchorError::InvalidDocument)
                        .and_then(|value| {
                            verification_evidence::rfc3339_instant(value)
                                .ok_or(AnchorError::InvalidDocument)
                        })
                })
                .transpose()?;
            if not_after.is_some_and(|not_after| issued_at.0 > not_after.1) {
                return Err(AnchorError::InvalidDocument);
            }
            Some(RevocationMetadata {
                list_version,
                issued_at,
                not_after,
            })
        }
        _ => return Err(AnchorError::InvalidDocument),
    };
    let revoked = match strict_json::field(object, "revoked_keys")
        .map_err(|_| AnchorError::InvalidDocument)?
    {
        JsonValue::Array(revoked) => revoked,
        _ => return Err(AnchorError::InvalidDocument),
    };
    let mut revoked_keys = RevocationKeySet::new();
    for key_id in revoked {
        let JsonValue::String(key_id) = key_id else {
            return Err(AnchorError::InvalidDocument);
        };
        if key_id.is_empty() {
            return Err(AnchorError::InvalidDocument);
        }
        revoked_keys.insert(key_id.clone());
    }
    Ok(RevocationList {
        revoked_keys,
        metadata,
    })
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SignatureBinding {
    pub envelope_match: bool,
    pub signature_valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_digest: Option<String>,
    pub subject_bound: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_pinned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_not_revoked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_authentic: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub envelope_fresh: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub produced_at_signed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revocation_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_authentic: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revocation_list_authentic: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revocation_list_current: Option<bool>,
    pub checks_not_performed: Vec<&'static str>,
}

impl SignatureBinding {
    pub fn failed_fields(&self) -> Vec<&'static str> {
        let mut failed = Vec::new();
        if !self.envelope_match {
            failed.push("envelope_match");
        }
        if !self.signature_valid {
            failed.push("signature_valid");
        }
        if !self.subject_bound {
            failed.push("subject_bound");
        }
        for (name, value) in [
            ("key_pinned", self.key_pinned),
            ("key_not_revoked", self.key_not_revoked),
            ("key_authentic", self.key_authentic),
            ("envelope_fresh", self.envelope_fresh),
            ("produced_at_signed", self.produced_at_signed),
            ("registry_authentic", self.registry_authentic),
            ("revocation_list_authentic", self.revocation_list_authentic),
            ("revocation_list_current", self.revocation_list_current),
        ] {
            if value == Some(false) {
                failed.push(name);
            }
        }
        failed
    }
}

#[derive(Clone, Copy)]
pub struct SignatureBindingInput<'a> {
    pub envelope: &'a Value,
    pub evidence_fingerprint: &'a str,
    pub producer_key_hex: &'a str,
    pub expected_producer_key: Option<&'a str>,
    pub reference_time: Option<(i128, i128)>,
    pub max_age_seconds: Option<u64>,
    pub signed_produced_at: Option<Option<&'a str>>,
    pub key_registry: Option<&'a ProducerKeyRegistry>,
    pub registry_digest: Option<&'a str>,
    pub registry_signature: Option<&'a Value>,
    pub registry_key_hex: Option<&'a str>,
    pub revocation_list: Option<&'a RevocationList>,
    pub revocation_digest: Option<&'a str>,
    pub revocation_signature: Option<&'a Value>,
    pub revocation_key_hex: Option<&'a str>,
    pub min_revocation_version: Option<u64>,
}

pub fn verify_binding(input: SignatureBindingInput<'_>) -> SignatureBinding {
    let matched = envelope_match(input.envelope);
    let envelope = input.envelope;
    let subject_digest = matched.then(|| envelope["subject_digest"].as_str().unwrap().to_owned());
    let signature_valid = matched
        && signature_valid(
            input.producer_key_hex,
            envelope["signature"].as_str().unwrap(),
            envelope["subject_digest"].as_str().unwrap().as_bytes(),
        );
    let subject_bound =
        matched && envelope["subject_digest"].as_str() == Some(input.evidence_fingerprint);
    let key_pinned = input
        .expected_producer_key
        .map(|expected| matched && envelope["producer_key_id"].as_str() == Some(expected));

    let registry_authentic = input.registry_signature.map(|wrapper| {
        matched && anchor_signature_valid(wrapper, input.registry_key_hex, input.registry_digest)
    });
    let revocation_list_authentic = input.revocation_signature.map(|wrapper| {
        anchor_signature_valid(wrapper, input.revocation_key_hex, input.revocation_digest)
    });

    let key_not_revoked = input.revocation_list.map(|revoked| {
        let not_listed = matched
            && envelope["producer_key_id"]
                .as_str()
                .is_some_and(|key_id| !revoked.revoked_keys.contains(key_id));
        not_listed && revocation_list_authentic.unwrap_or(true)
    });
    let key_authentic = input.key_registry.map(|registry| {
        let in_window = matched
            && input.reference_time.is_some_and(|reference| {
                envelope["producer_key_id"]
                    .as_str()
                    .and_then(|key_id| registry.get(key_id))
                    .is_some_and(|entry| {
                        entry.not_before.1 <= reference.0 && entry.not_after.0 >= reference.1
                    })
            });
        in_window && registry_authentic.unwrap_or(true)
    });
    let envelope_fresh = input.max_age_seconds.map(|max_age| {
        matched
            && input.reference_time.is_some_and(|reference| {
                envelope["produced_at"]
                    .as_str()
                    .and_then(verification_evidence::rfc3339_instant)
                    .is_some_and(|produced| {
                        produced.1 <= reference.0
                            && reference.1 - produced.0 <= i128::from(max_age) * 1_000_000_000
                    })
            })
    });
    let produced_at_signed = input.signed_produced_at.flatten().map(|timestamp| {
        matched
            && signature_valid
            && subject_bound
            && envelope["produced_at"].as_str() == Some(timestamp)
    });
    let revocation_list_current = input.revocation_list.and_then(|list| {
        let metadata = list.metadata?;
        let minimum_version = input.min_revocation_version?;
        let reference_time = input.reference_time?;
        Some(
            metadata.issued_at.1 <= reference_time.0
                && metadata
                    .not_after
                    .is_none_or(|not_after| not_after.0 >= reference_time.1)
                && metadata.list_version >= minimum_version
                && revocation_list_authentic == Some(true),
        )
    });

    let mut checks_not_performed = vec![
        "key_binding",
        "key_authenticity",
        "envelope_freshness",
        "key_revocation",
        "registry_authenticity",
        "revocation_list_authenticity",
        "revocation_list_rollback",
        "producer_time_authenticity",
        "clock_authenticity",
    ];
    if input.expected_producer_key.is_some() {
        checks_not_performed.retain(|check| *check != "key_binding");
    }
    if input.key_registry.is_some() {
        checks_not_performed.retain(|check| *check != "key_authenticity");
    }
    if input.max_age_seconds.is_some() {
        checks_not_performed.retain(|check| *check != "envelope_freshness");
    }
    if input.revocation_list.is_some() {
        checks_not_performed.retain(|check| *check != "key_revocation");
    }
    if input.registry_signature.is_some() {
        checks_not_performed.retain(|check| *check != "registry_authenticity");
    }
    if input.revocation_signature.is_some() {
        checks_not_performed.retain(|check| *check != "revocation_list_authenticity");
    }
    if revocation_list_current.is_some() {
        checks_not_performed.retain(|check| *check != "revocation_list_rollback");
    }
    if produced_at_signed.is_some() {
        checks_not_performed.retain(|check| *check != "producer_time_authenticity");
    }

    SignatureBinding {
        envelope_match: matched,
        signature_valid,
        subject_digest,
        subject_bound,
        key_pinned,
        key_not_revoked,
        key_authentic,
        envelope_fresh,
        produced_at_signed,
        registry_digest: input.registry_digest.map(str::to_owned),
        revocation_digest: input.revocation_digest.map(str::to_owned),
        registry_authentic,
        revocation_list_authentic,
        revocation_list_current,
        checks_not_performed,
    }
}

fn anchor_signature_valid(
    envelope: &Value,
    public_key_hex: Option<&str>,
    expected_digest: Option<&str>,
) -> bool {
    if !envelope_match(envelope) {
        return false;
    }
    let (Some(public_key_hex), Some(expected_digest)) = (public_key_hex, expected_digest) else {
        return false;
    };
    let Some(subject_digest) = envelope["subject_digest"].as_str() else {
        return false;
    };
    subject_digest == expected_digest
        && signature_valid(
            public_key_hex,
            envelope["signature"].as_str().unwrap(),
            subject_digest.as_bytes(),
        )
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn decode_lower_hex<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 {
        return None;
    }
    let mut decoded = [0u8; N];
    for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let high = lower_hex_nibble(pair[0])?;
        let low = lower_hex_nibble(pair[1])?;
        decoded[index] = high << 4 | low;
    }
    Some(decoded)
}

fn lower_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Sign `record` under `secret_key` and emit the detached envelope.
/// The signature covers the digest's lowercase-hex ASCII bytes.
pub fn sign(
    record: &[u8],
    secret_key: &SigningKey,
    producer_key_id: &str,
    produced_at: Option<&str>,
) -> Result<Envelope, SignError> {
    if producer_key_id.is_empty() {
        return Err(SignError::InvalidProducerKeyId);
    }
    if let Some(value) = produced_at {
        if !verification_evidence::is_rfc3339_timestamp(value) {
            return Err(SignError::InvalidProducedAt);
        }
    }
    let digest = subject_digest(record);
    let signature = secret_key.sign(digest.as_bytes());
    Ok(Envelope {
        schema_version: PRODUCER_SIGNATURE_SCHEMA,
        subject_digest: digest,
        algorithm: "ed25519",
        signature: hex(&signature.to_bytes()),
        producer_key_id: producer_key_id.to_string(),
        produced_at: produced_at.map(str::to_owned),
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
