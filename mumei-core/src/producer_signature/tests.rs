use super::*;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde_json::{json, Value};
use std::collections::BTreeSet;

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
    for bad in [
        "yesterday",
        "2026-01-01",
        "2026-01-01T00:00:00",
        "2026-02-30T00:00:00Z",
        "2025-02-29T00:00:00Z",
        "2026-04-31T00:00:00Z",
        "2016-12-31T23:59:60Z",
    ] {
        assert_eq!(
            sign(b"{}", &key, "producer-a", Some(bad)).unwrap_err(),
            SignError::InvalidProducedAt
        );
    }
}

struct BindingFixture {
    producer_public: String,
    root_public: String,
    root_secret: String,
    envelope: Value,
    evidence_fingerprint: String,
    registry: ProducerKeyRegistry,
    registry_bytes: Vec<u8>,
    registry_digest: String,
    registry_signature: Value,
    revoked: RevocationList,
    revocation_bytes: Vec<u8>,
    revocation_digest: String,
    revocation_signature: Value,
    reference_time: (i128, i128),
}

impl BindingFixture {
    fn new() -> Self {
        let (producer_secret, producer_public) = generate_keypair();
        let producer_signing_key = decode_secret_key(&producer_secret).unwrap();
        let record = b"binding fixture evidence";
        let envelope = serde_json::to_value(
            sign(
                record,
                &producer_signing_key,
                "producer-a",
                Some("2024-02-01T00:00:00Z"),
            )
            .unwrap(),
        )
        .unwrap();
        let evidence_fingerprint = subject_digest(record);

        let (root_secret, root_public) = generate_keypair();
        let root_signing_key = decode_secret_key(&root_secret).unwrap();
        let registry_bytes = serde_json::to_vec(&json!({
            "schema_version": PRODUCER_KEY_REGISTRY_SCHEMA,
            "registry": {
                "producer-a": {
                    "principal": "test-principal",
                    "not_before": "2024-01-01T00:00:00Z",
                    "not_after": "2024-12-31T23:59:59Z",
                }
            }
        }))
        .unwrap();
        let registry = parse_key_registry(&registry_bytes).unwrap();
        let registry_digest = subject_digest(&registry_bytes);
        let registry_signature =
            serde_json::to_value(sign(&registry_bytes, &root_signing_key, "root", None).unwrap())
                .unwrap();

        let revocation_bytes = serde_json::to_vec(&json!({
            "schema_version": KEY_REVOCATION_LIST_SCHEMA,
            "revoked_keys": [],
        }))
        .unwrap();
        let revoked = parse_revocation_list(&revocation_bytes).unwrap();
        let revocation_digest = subject_digest(&revocation_bytes);
        let revocation_signature =
            serde_json::to_value(sign(&revocation_bytes, &root_signing_key, "root", None).unwrap())
                .unwrap();

        Self {
            producer_public,
            root_public,
            root_secret,
            envelope,
            evidence_fingerprint,
            registry,
            registry_bytes,
            registry_digest,
            registry_signature,
            revoked,
            revocation_bytes,
            revocation_digest,
            revocation_signature,
            reference_time: verification_evidence::rfc3339_instant("2024-02-15T00:00:00Z").unwrap(),
        }
    }

    fn input(&self) -> SignatureBindingInput<'_> {
        SignatureBindingInput {
            envelope: &self.envelope,
            evidence_fingerprint: &self.evidence_fingerprint,
            producer_key_hex: &self.producer_public,
            expected_producer_key: Some("producer-a"),
            reference_time: Some(self.reference_time),
            max_age_seconds: Some(30 * 24 * 60 * 60),
            signed_produced_at: None,
            key_registry: Some(&self.registry),
            registry_digest: Some(&self.registry_digest),
            registry_signature: Some(&self.registry_signature),
            registry_key_hex: Some(&self.root_public),
            revocation_list: Some(&self.revoked),
            revocation_digest: Some(&self.revocation_digest),
            revocation_signature: Some(&self.revocation_signature),
            revocation_key_hex: Some(&self.root_public),
            min_revocation_version: None,
        }
    }
}

#[test]
fn signature_envelopes_and_public_keys_use_closed_lowercase_hex_shapes() {
    let fixture = BindingFixture::new();
    assert!(envelope_match(&fixture.envelope));

    let mut extra = fixture.envelope.clone();
    extra["unexpected"] = json!(true);
    assert!(!envelope_match(&extra));
    assert!(!envelope_match(&json!([])));

    let mut malformed_stamp = fixture.envelope.clone();
    malformed_stamp["produced_at"] = Value::Null;
    assert!(!envelope_match(&malformed_stamp));

    assert!(decode_public_key_hex(&fixture.producer_public).is_some());
    for invalid in [
        String::new(),
        "00".repeat(31),
        "AA".repeat(32),
        "zz".repeat(32),
        format!("{} ", fixture.producer_public),
    ] {
        assert!(decode_public_key_hex(&invalid).is_none());
    }
}

#[test]
fn signature_verification_rejects_wrong_or_invalid_public_keys() {
    let fixture = BindingFixture::new();
    let digest = fixture.envelope["subject_digest"].as_str().unwrap();
    let signature = fixture.envelope["signature"].as_str().unwrap();
    assert!(signature_valid(
        &fixture.producer_public,
        signature,
        digest.as_bytes()
    ));
    let (_, wrong_public) = generate_keypair();
    assert!(!signature_valid(
        &wrong_public,
        signature,
        digest.as_bytes()
    ));
    assert!(!signature_valid(
        &"ff".repeat(32),
        signature,
        digest.as_bytes()
    ));
    assert!(!signature_valid(
        &fixture.producer_public,
        "AA".repeat(64).as_str(),
        digest.as_bytes()
    ));
}

#[test]
fn trust_anchor_parsers_validate_closed_shapes_and_reject_duplicate_keys() {
    let fixture = BindingFixture::new();
    assert_eq!(
        parse_key_registry(&fixture.registry_bytes).unwrap(),
        fixture.registry
    );
    assert_eq!(
        parse_revocation_list(&fixture.revocation_bytes).unwrap(),
        fixture.revoked
    );

    for duplicate in [
        br#"{"schema_version":"mumei.producer-key-registry/v1","schema_version":"mumei.producer-key-registry/v1","registry":{"producer-a":{"principal":"p","not_before":"2024-01-01T00:00:00Z","not_after":"2024-12-31T00:00:00Z"}}}"#.as_slice(),
        br#"{"schema_version":"mumei.producer-key-registry/v1","registry":{"producer-a":{"principal":"p","principal":"q","not_before":"2024-01-01T00:00:00Z","not_after":"2024-12-31T00:00:00Z"}}}"#.as_slice(),
        br#"{"schema_version":"mumei.producer-key-registry/v1","registry":{"producer-a":{"principal":"p","not_before":"2024-01-01T00:00:00Z","not_after":"2024-12-31T00:00:00Z"}},"unknown":{"same":1,"same":2}}"#.as_slice(),
    ] {
        assert_eq!(
            parse_key_registry(duplicate),
            Err(AnchorError::InvalidDocument)
        );
    }
    for malformed in [
        br#"{"schema_version":"mumei.producer-key-registry/v1","registry":{}}"#.as_slice(),
        br#"{"schema_version":"mumei.producer-key-registry/v1","registry":{"":{"principal":"p","not_before":"2024-01-01T00:00:00Z","not_after":"2024-12-31T00:00:00Z"}}}"#.as_slice(),
        br#"{"schema_version":"mumei.producer-key-registry/v1","registry":{"producer-a":{"principal":"","not_before":"2024-01-01T00:00:00Z","not_after":"2024-12-31T00:00:00Z"}}}"#.as_slice(),
        br#"{"schema_version":"mumei.producer-key-registry/v1","registry":{"producer-a":{"principal":"p","not_before":"2024-12-31T00:00:00Z","not_after":"2024-01-01T00:00:00Z"}}}"#.as_slice(),
        br#"{"schema_version":"mumei.producer-key-registry/v1","registry":{"producer-a":{"principal":"p","not_before":"2024-02-30T00:00:00Z","not_after":"2024-12-31T00:00:00Z"}}}"#.as_slice(),
        br#"{"schema_version":"mumei.producer-key-registry/v1","registry":{"producer-a":{"principal":"p","not_before":"2024-01-01T00:00:00Z","not_after":"2024-12-31T00:00:00Z","extra":true}}}"#.as_slice(),
        br#"{"schema_version":"mumei.producer-key-registry/v1","registry":{"producer-a":{"principal":"p","not_before":"2024-01-01T00:00:00Z","not_after":"2024-12-31T00:00:00Z"}},"extra":true}"#.as_slice(),
    ] {
        assert_eq!(
            parse_key_registry(malformed),
            Err(AnchorError::InvalidDocument)
        );
    }
    assert_eq!(
        parse_revocation_list(
            br#"{"schema_version":"mumei.key-revocation-list/v1","revoked_keys":["producer-a",""]}"#
        ),
        Err(AnchorError::InvalidDocument)
    );
    assert_eq!(
        parse_revocation_list(
            br#"{"schema_version":"mumei.key-revocation-list/v1","revoked_keys":[],"extra":true}"#
        ),
        Err(AnchorError::InvalidDocument)
    );
}

#[test]
fn revocation_list_v2_parser_preserves_v1_and_returns_closed_metadata() {
    let v1 = parse_revocation_list(
        br#"{"schema_version":"mumei.key-revocation-list/v1","revoked_keys":["producer-a"]}"#,
    )
    .unwrap();
    assert_eq!(v1.revoked_keys, BTreeSet::from(["producer-a".to_owned()]));
    assert_eq!(v1.metadata, None);

    let v2 = parse_revocation_list(
        br#"{"schema_version":"mumei.key-revocation-list/v2","list_version":7,"issued_at":"2024-02-01T00:00:00Z","not_after":"2024-03-01T00:00:00Z","revoked_keys":["producer-a"]}"#,
    )
    .unwrap();
    assert_eq!(v2.revoked_keys, BTreeSet::from(["producer-a".to_owned()]));
    assert_eq!(
        v2.metadata,
        Some(RevocationMetadata {
            list_version: 7,
            issued_at: verification_evidence::rfc3339_instant("2024-02-01T00:00:00Z").unwrap(),
            not_after: Some(
                verification_evidence::rfc3339_instant("2024-03-01T00:00:00Z").unwrap()
            ),
        })
    );

    let no_expiry = parse_revocation_list(
        br#"{"schema_version":"mumei.key-revocation-list/v2","list_version":1,"issued_at":"2024-02-01T00:00:00Z","revoked_keys":[]}"#,
    )
    .unwrap();
    assert_eq!(no_expiry.metadata.unwrap().not_after, None);
}

#[test]
fn revocation_list_v2_parser_rejects_invalid_versions_windows_and_shapes() {
    for malformed in [
        br#"{"schema_version":"mumei.key-revocation-list/v2","list_version":0,"issued_at":"2024-02-01T00:00:00Z","revoked_keys":[]}"#.as_slice(),
        br#"{"schema_version":"mumei.key-revocation-list/v2","list_version":-1,"issued_at":"2024-02-01T00:00:00Z","revoked_keys":[]}"#.as_slice(),
        br#"{"schema_version":"mumei.key-revocation-list/v2","list_version":1.0,"issued_at":"2024-02-01T00:00:00Z","revoked_keys":[]}"#.as_slice(),
        br#"{"schema_version":"mumei.key-revocation-list/v2","list_version":true,"issued_at":"2024-02-01T00:00:00Z","revoked_keys":[]}"#.as_slice(),
        br#"{"schema_version":"mumei.key-revocation-list/v2","list_version":1,"revoked_keys":[]}"#.as_slice(),
        br#"{"schema_version":"mumei.key-revocation-list/v2","list_version":1,"issued_at":"2024-02-01T00:00:00Z","revoked_keys":[],"extra":true}"#.as_slice(),
        br#"{"schema_version":"mumei.key-revocation-list/v2","list_version":1,"issued_at":"2024-03-01T00:00:00Z","not_after":"2024-02-01T00:00:00Z","revoked_keys":[]}"#.as_slice(),
        br#"{"schema_version":"mumei.key-revocation-list/v2","list_version":1,"issued_at":"2024-02-30T00:00:00Z","revoked_keys":[]}"#.as_slice(),
        br#"{"schema_version":"mumei.key-revocation-list/v2","list_version":1,"issued_at":"2024-02-01T00:00:00Z","not_after":null,"revoked_keys":[]}"#.as_slice(),
        br#"{"schema_version":"mumei.key-revocation-list/v2","list_version":1,"list_version":2,"issued_at":"2024-02-01T00:00:00Z","revoked_keys":[]}"#.as_slice(),
    ] {
        assert_eq!(
            parse_revocation_list(malformed),
            Err(AnchorError::InvalidDocument)
        );
    }
}

#[test]
fn signature_binding_emits_true_results_for_a_valid_complete_chain() {
    let fixture = BindingFixture::new();
    let binding = verify_binding(fixture.input());
    assert!(binding.envelope_match);
    assert!(binding.signature_valid);
    assert_eq!(
        binding.subject_digest.as_deref(),
        Some(fixture.evidence_fingerprint.as_str())
    );
    assert!(binding.subject_bound);
    assert_eq!(binding.key_pinned, Some(true));
    assert_eq!(binding.key_not_revoked, Some(true));
    assert_eq!(binding.key_authentic, Some(true));
    assert_eq!(binding.envelope_fresh, Some(true));
    assert_eq!(binding.registry_authentic, Some(true));
    assert_eq!(binding.revocation_list_authentic, Some(true));
    assert_eq!(binding.revocation_list_current, None);
    assert!(binding.failed_fields().is_empty());
    assert_eq!(
        binding.checks_not_performed,
        [
            "revocation_list_rollback",
            "producer_time_authenticity",
            "clock_authenticity"
        ]
    );
}

#[test]
fn current_v2_revocation_list_is_independent_of_a_mismatched_signature_envelope() {
    let fixture = BindingFixture::new();
    let revocation_bytes =
        br#"{"schema_version":"mumei.key-revocation-list/v2","list_version":1,"issued_at":"2024-02-01T00:00:00Z","revoked_keys":[]}"#;
    let revocation_list = parse_revocation_list(revocation_bytes).unwrap();
    let revocation_digest = subject_digest(revocation_bytes);
    let revocation_key = decode_secret_key(&fixture.root_secret).unwrap();
    let revocation_signature =
        serde_json::to_value(sign(revocation_bytes, &revocation_key, "root", None).unwrap())
            .unwrap();
    let mismatched_envelope = json!({
        "schema_version": PRODUCER_SIGNATURE_SCHEMA,
        "unexpected": true,
    });

    let mut input = fixture.input();
    input.envelope = &mismatched_envelope;
    input.revocation_list = Some(&revocation_list);
    input.revocation_digest = Some(&revocation_digest);
    input.revocation_signature = Some(&revocation_signature);
    input.min_revocation_version = Some(1);
    let binding = verify_binding(input);

    assert!(!binding.envelope_match);
    assert_eq!(binding.revocation_list_authentic, Some(true));
    assert_eq!(binding.revocation_list_current, Some(true));
    assert!(binding.failed_fields().contains(&"envelope_match"));
    assert!(binding.failed_fields().contains(&"signature_valid"));
}

#[test]
fn v2_revocation_list_current_requires_root_authenticity() {
    let fixture = BindingFixture::new();
    let revocation_bytes = br#"{"schema_version":"mumei.key-revocation-list/v2","list_version":1,"issued_at":"2024-02-01T00:00:00Z","revoked_keys":[]}"#;
    let revocation_list = parse_revocation_list(revocation_bytes).unwrap();
    let revocation_digest = subject_digest(revocation_bytes);
    let mut invalid_signature = fixture.revocation_signature.clone();
    invalid_signature["subject_digest"] = json!(revocation_digest);
    invalid_signature["signature"] = json!("0".repeat(128));

    let mut input = fixture.input();
    input.revocation_list = Some(&revocation_list);
    input.revocation_digest = Some(&revocation_digest);
    input.revocation_signature = Some(&invalid_signature);
    input.min_revocation_version = Some(1);
    let binding = verify_binding(input);

    assert_eq!(binding.revocation_list_authentic, Some(false));
    assert_eq!(binding.revocation_list_current, Some(false));
    assert!(binding.failed_fields().contains(&"revocation_list_current"));
}

#[test]
fn signed_produced_at_requires_a_declared_matching_record_timestamp_and_all_binding_gates() {
    let fixture = BindingFixture::new();
    let timestamp = "2024-02-01T00:00:00Z";

    let mut input = fixture.input();
    input.signed_produced_at = Some(Some(timestamp));
    let binding = verify_binding(input);
    assert_eq!(binding.produced_at_signed, Some(true));
    assert!(!binding
        .checks_not_performed
        .contains(&"producer_time_authenticity"));
    assert!(binding.checks_not_performed.contains(&"clock_authenticity"));
    assert!(!binding.failed_fields().contains(&"produced_at_signed"));

    for signed_produced_at in [None, Some(None)] {
        let mut input = fixture.input();
        input.signed_produced_at = signed_produced_at;
        let binding = verify_binding(input);
        assert_eq!(binding.produced_at_signed, None);
        assert!(binding
            .checks_not_performed
            .contains(&"producer_time_authenticity"));
        assert!(serde_json::to_value(binding)
            .unwrap()
            .get("produced_at_signed")
            .is_none());
    }

    let mut input = fixture.input();
    input.signed_produced_at = Some(Some("2024-02-02T00:00:00Z"));
    let binding = verify_binding(input);
    assert_eq!(binding.produced_at_signed, Some(false));
    assert!(binding.failed_fields().contains(&"produced_at_signed"));
    assert!(!binding
        .checks_not_performed
        .contains(&"producer_time_authenticity"));
    assert!(binding.checks_not_performed.contains(&"clock_authenticity"));

    let mut input = fixture.input();
    input.producer_key_hex = &fixture.root_public;
    input.signed_produced_at = Some(Some(timestamp));
    assert_eq!(verify_binding(input).produced_at_signed, Some(false));

    let wrong_fingerprint = "0".repeat(64);
    let mut input = fixture.input();
    input.evidence_fingerprint = &wrong_fingerprint;
    input.signed_produced_at = Some(Some(timestamp));
    assert_eq!(verify_binding(input).produced_at_signed, Some(false));

    let mut missing_stamp = fixture.envelope.clone();
    missing_stamp.as_object_mut().unwrap().remove("produced_at");
    let mut input = fixture.input();
    input.envelope = &missing_stamp;
    input.signed_produced_at = Some(Some(timestamp));
    assert_eq!(verify_binding(input).produced_at_signed, Some(false));
}

#[test]
fn signature_binding_reports_each_failed_binding_check() {
    let fixture = BindingFixture::new();
    let mut input = fixture.input();
    input.producer_key_hex = &fixture.root_public;
    assert_eq!(verify_binding(input).failed_fields(), ["signature_valid"]);

    let wrong_fingerprint = "0".repeat(64);
    let mut input = fixture.input();
    input.evidence_fingerprint = &wrong_fingerprint;
    assert!(!verify_binding(input).subject_bound);

    let mut input = fixture.input();
    input.expected_producer_key = Some("other-producer");
    assert_eq!(verify_binding(input).key_pinned, Some(false));

    let revoked = RevocationList {
        revoked_keys: BTreeSet::from(["producer-a".to_owned()]),
        metadata: None,
    };
    let mut input = fixture.input();
    input.revocation_list = Some(&revoked);
    input.revocation_digest = None;
    input.revocation_signature = None;
    input.revocation_key_hex = None;
    assert_eq!(verify_binding(input).key_not_revoked, Some(false));

    let after_window = verification_evidence::rfc3339_instant("2025-01-01T00:00:00Z").unwrap();
    let mut input = fixture.input();
    input.reference_time = Some(after_window);
    assert_eq!(verify_binding(input).key_authentic, Some(false));

    let mut input = fixture.input();
    input.max_age_seconds = Some(1);
    assert_eq!(verify_binding(input).envelope_fresh, Some(false));

    let mut bad_registry_signature = fixture.registry_signature.clone();
    bad_registry_signature["unexpected"] = json!(true);
    let mut input = fixture.input();
    input.registry_signature = Some(&bad_registry_signature);
    let binding = verify_binding(input);
    assert_eq!(binding.registry_authentic, Some(false));
    assert_eq!(binding.key_authentic, Some(false));

    let mut bad_revocation_signature = fixture.revocation_signature.clone();
    bad_revocation_signature["unexpected"] = json!(true);
    let mut input = fixture.input();
    input.revocation_signature = Some(&bad_revocation_signature);
    let binding = verify_binding(input);
    assert_eq!(binding.revocation_list_authentic, Some(false));
    assert_eq!(binding.key_not_revoked, Some(false));

    let mut bad_envelope = fixture.envelope.clone();
    bad_envelope["unexpected"] = json!(true);
    let mut input = fixture.input();
    input.envelope = &bad_envelope;
    let binding = verify_binding(input);
    assert!(!binding.envelope_match);
    assert!(!binding.signature_valid);
    assert_eq!(binding.subject_digest, None);
    assert!(!binding.subject_bound);
    assert_eq!(binding.key_pinned, Some(false));
    assert_eq!(binding.key_not_revoked, Some(false));
    assert_eq!(binding.key_authentic, Some(false));
    assert_eq!(binding.envelope_fresh, Some(false));
    assert_eq!(binding.registry_authentic, Some(false));
    assert_eq!(binding.revocation_list_authentic, Some(true));
}
