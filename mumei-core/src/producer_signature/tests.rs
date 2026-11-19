use super::*;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde_json::Value;

// Obviously fake sentinel key material (AGENTS.md: never a real-looking secret).
const SECRET: &str = "1111111111111111111111111111111111111111111111111111111111111111";

fn envelope(record: &[u8]) -> Value {
    let key = decode_secret_key(SECRET).unwrap();
    serde_json::to_value(sign(record, &key, "producer-a", None).unwrap()).unwrap()
}

#[test]
fn signed_envelope_verifies_against_the_public_key() {
    let key = decode_secret_key(SECRET).unwrap();
    let record = b"{\"claim\": 1}";
    let signed = sign(record, &key, "producer-a", None).unwrap();
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
    let other = sign(record, &key, "producer-b", None).unwrap();
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
        sign(b"{}", &key, "", None).unwrap_err(),
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

#[test]
fn generated_keypair_signs_and_verifies() {
    let (secret_hex, public_hex) = generate_keypair();
    assert_eq!(secret_hex.len(), 64);
    assert_eq!(public_hex.len(), 64);
    assert!(secret_hex
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    assert!(public_hex
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));

    let key = decode_secret_key(&secret_hex).unwrap();
    let record = b"{\"claim\": 2}";
    let signed = sign(record, &key, "producer-b", None).unwrap();
    let public = VerifyingKey::from_bytes(&decode_hex(&public_hex).try_into().unwrap()).unwrap();
    let signature = Signature::from_bytes(&decode_hex(&signed.signature).try_into().unwrap());
    public
        .verify(signed.subject_digest.as_bytes(), &signature)
        .unwrap();

    // Fresh entropy: two keypairs never collide.
    assert_ne!(generate_keypair(), (secret_hex, public_hex));
}

fn decode_hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
        .collect()
}

#[test]
fn produced_at_joins_the_envelope_only_when_stamped() {
    let key = decode_secret_key(SECRET).unwrap();
    let record = b"{\"claim\": 1}";

    // Unstamped envelopes keep the closed v1 field set.
    let plain = serde_json::to_value(sign(record, &key, "producer-a", None).unwrap()).unwrap();
    assert!(plain.get("produced_at").is_none());

    // A stamped timestamp lands on the envelope and the signature is
    // unaffected — the signed payload is the digest alone.
    let stamped = sign(record, &key, "producer-a", Some("2026-01-01T00:00:00Z")).unwrap();
    let value = serde_json::to_value(&stamped).unwrap();
    assert_eq!(value["produced_at"], "2026-01-01T00:00:00Z");
    assert_eq!(
        stamped.signature,
        sign(record, &key, "producer-a", None).unwrap().signature
    );
}

#[test]
fn non_rfc3339_produced_at_is_rejected() {
    let key = decode_secret_key(SECRET).unwrap();
    for bad in ["yesterday", "2026-01-01", "2026-01-01T00:00:00"] {
        assert_eq!(
            sign(b"{}", &key, "producer-a", Some(bad)).unwrap_err(),
            SignError::InvalidProducedAt
        );
    }
}
