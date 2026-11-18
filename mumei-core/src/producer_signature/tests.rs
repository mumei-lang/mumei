use super::*;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde_json::Value;

// Obviously fake sentinel key material (AGENTS.md: never a real-looking secret).
const SECRET: &str = "1111111111111111111111111111111111111111111111111111111111111111";

fn envelope(record: &[u8]) -> Value {
    let key = decode_secret_key(SECRET).unwrap();
    serde_json::to_value(sign(record, &key, "producer-a").unwrap()).unwrap()
}

#[test]
fn signed_envelope_verifies_against_the_public_key() {
    let key = decode_secret_key(SECRET).unwrap();
    let record = b"{\"claim\": 1}";
    let signed = sign(record, &key, "producer-a").unwrap();
    let value = serde_json::to_value(&signed).unwrap();
    assert_eq!(value["schema_version"], PRODUCER_SIGNATURE_SCHEMA);
    assert_eq!(value["algorithm"], "ed25519");
    assert_eq!(value["producer_key_id"], "producer-a");
    assert_eq!(value["subject_digest"], subject_digest(record));
    assert_eq!(value["signature"].as_str().unwrap().len(), 128);

    // The consumer's verify convention: public key over the digest's
    // ASCII bytes (crypto_verify_signature signs message.as_bytes()).
    let public = VerifyingKey::from_bytes(&key.verifying_key().to_bytes()).unwrap();
    let signature = Signature::from_bytes(
        &decode_hex(value["signature"].as_str().unwrap())
            .try_into()
            .unwrap(),
    );
    public
        .verify(
            value["subject_digest"].as_str().unwrap().as_bytes(),
            &signature,
        )
        .unwrap();

    // The same record under a different key id signs identically (the
    // label is declared, never derived).
    let other = sign(record, &key, "producer-b").unwrap();
    assert_eq!(signed.signature, other.signature);
}

#[test]
fn digest_matches_sha256_of_the_raw_record_bytes() {
    let record = b"raw bytes \x00\xff not utf8";
    assert_eq!(
        subject_digest(record),
        format!("{:x}", Sha256::digest(record))
    );
    // whitespace-sensitive: a byte difference changes the digest
    assert_ne!(subject_digest(b"a {}"), subject_digest(b"a{}"));
}

#[test]
fn envelope_shape_is_exactly_the_closed_v1_fields() {
    let value = envelope(b"{}");
    let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "algorithm",
            "producer_key_id",
            "schema_version",
            "signature",
            "subject_digest"
        ]
    );
}

#[test]
fn empty_producer_key_id_is_rejected() {
    let key = decode_secret_key(SECRET).unwrap();
    assert_eq!(
        sign(b"{}", &key, "").unwrap_err(),
        SignError::InvalidProducerKeyId
    );
}

#[test]
fn key_material_must_be_64_lowercase_hex() {
    for bad in [
        String::new(),
        "0".to_string(),                  // short
        "ab".repeat(33),                  // 66 chars
        "AB".repeat(32),                  // uppercase
        "zz".repeat(32),                  // non-hex
        format!(" {}", "ab".repeat(32)),  // leading space
        format!("{}\n", "ab".repeat(32)), // trailing newline
        format!("0x{}", "ab".repeat(32)), // prefix
    ] {
        assert_eq!(
            decode_secret_key(bad.as_str()).unwrap_err(),
            SignError::InvalidKeyMaterial,
            "key: {bad:?}"
        );
    }
    assert!(decode_secret_key(SECRET).is_ok());
    assert!(decode_secret_key(&"00".repeat(32)).is_ok());
}

#[test]
fn owned_key_bytes_decode_and_are_consumed() {
    let key = decode_secret_key_bytes(SECRET.as_bytes().to_vec()).unwrap();
    assert_eq!(
        key.verifying_key(),
        decode_secret_key(SECRET).unwrap().verifying_key()
    );
    assert_eq!(
        decode_secret_key_bytes(vec![0xff; 64]).unwrap_err(),
        SignError::InvalidKeyMaterial
    );
    assert_eq!(
        decode_secret_key_bytes(b"too short".to_vec()).unwrap_err(),
        SignError::InvalidKeyMaterial
    );
}

fn decode_hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
        .collect()
}
