use super::*;
use serde_json::{json, Value};
use std::error::Error;

const FIXTURES: &str =
    include_str!("../../../tests/fixtures/verification-evidence/synthetic_cases.json");
const NATIVE: &str =
    include_str!("../../../tests/fixtures/verification-evidence/native_consumer_shape.json");
const SCHEMA: &str = include_str!("../../../schema/verification-evidence.schema.json");
const MATRIX: &str =
    include_str!("../../../tests/fixtures/verification-evidence/representation_matrix.json");

fn base() -> Value {
    serde_json::from_str::<Value>(FIXTURES).unwrap()["cases"][0]["evidence"].clone()
}

fn accepts(value: &Value) -> bool {
    decode(&serde_json::to_vec(value).unwrap()).is_ok()
}

fn check_rejected(value: &Value) {
    let error = decode(&serde_json::to_vec(value).unwrap()).unwrap_err();
    assert!(!format!("{error:?}: {error}").contains("FAKE_PRIVATE"));
    assert!(error.source().is_none());
}

#[test]
fn synthetic_and_actual_native_shapes_roundtrip_without_adding_acceptance() {
    let fixtures: Value = serde_json::from_str(FIXTURES).unwrap();
    let native: Value = serde_json::from_str(NATIVE).unwrap();
    for value in fixtures["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| &case["evidence"])
        .chain(native["evidence"].as_array().unwrap())
    {
        let evidence = decode(&serde_json::to_vec(value).unwrap()).unwrap();
        assert_eq!(&serde_json::to_value(&evidence).unwrap(), value);
        evidence.validate().unwrap();
        assert!(value.get("acceptance").is_none());
    }
}

fn enum_values(schema: &Value, path: &str) -> Vec<Value> {
    let mut node = schema;
    for key in path.split('.') {
        node = &node["properties"][key];
    }
    node["enum"].as_array().unwrap().clone()
}

fn combinations(schema: &Value, paths: &[&str]) -> Vec<Vec<Value>> {
    let mut rows = vec![Vec::new()];
    for path in paths {
        rows = rows
            .into_iter()
            .flat_map(|row| {
                enum_values(schema, path).into_iter().map(move |value| {
                    let mut row = row.clone();
                    row.push(value);
                    row
                })
            })
            .collect();
    }
    rows
}

#[test]
fn all_outcome_scope_and_correspondence_combinations_match_the_schema_oracle() {
    let schema: Value = serde_json::from_str(SCHEMA).unwrap();
    let oracle: Value = serde_json::from_str(MATRIX).unwrap();
    assert_eq!(oracle["schema_version"], SCHEMA_VERSION);
    for (section, paths) in [
        (
            "outcomes",
            vec!["method", "execution", "result", "scope.kind"],
        ),
        (
            "correspondence",
            vec!["correspondence.relation", "correspondence.status", "result"],
        ),
    ] {
        let cases = combinations(&schema, &paths);
        assert_eq!(
            cases.len() as u64,
            oracle["case_counts"][section].as_u64().unwrap()
        );
        let valid: HashSet<u64> = oracle["valid_indices"][section]
            .as_array()
            .unwrap()
            .iter()
            .map(|index| index.as_u64().unwrap())
            .collect();
        for (index, values) in cases.iter().enumerate() {
            let mut value = base();
            for (path, field) in paths.iter().zip(values) {
                *value
                    .pointer_mut(&format!("/{}", path.replace('.', "/")))
                    .unwrap() = field.clone();
            }
            if section == "correspondence" {
                value["method"] = json!(match value["result"].as_str().unwrap() {
                    "observed_no_failure" => "finite_execution",
                    "finding" => "abstract_interpretation",
                    _ => "deductive_proof",
                });
                value["scope"]["kind"] = json!(if value["method"] == "finite_execution" {
                    "finite"
                } else {
                    "unbounded"
                });
            }
            value["scope"]["bounds"] = if matches!(
                value["scope"]["kind"].as_str().unwrap(),
                "bounded" | "finite"
            ) {
                json!(["synthetic_limit"])
            } else {
                json!([])
            };
            assert_eq!(
                accepts(&value),
                valid.contains(&(index as u64)),
                "{section} {index}: {values:?}"
            );
        }
    }
}

#[test]
fn all_required_fields_and_closed_objects_include_nullable_target() {
    let original = base();
    for path in [
        "",
        "/subject",
        "/specification",
        "/semantics",
        "/scope",
        "/correspondence",
        "/provenance",
        "/provenance/adapter",
        "/provenance/tool",
    ] {
        let mut unknown = original.clone();
        unknown
            .pointer_mut(path)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("FAKE_PRIVATE_UNKNOWN".into(), json!("FAKE_PRIVATE_VALUE"));
        check_rejected(&unknown);
        for key in original.pointer(path).unwrap().as_object().unwrap().keys() {
            let mut missing = original.clone();
            missing
                .pointer_mut(path)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(key);
            check_rejected(&missing);
        }
    }
    let mut target = original.clone();
    target["semantics"]["target_triple"] = json!(null);
    assert!(accepts(&target));
    target["semantics"]["target_triple"] = json!("FAKE_PRIVATE_NONEMPTY_TARGET");
    assert!(accepts(&target));
    target["semantics"]["target_triple"] = json!("");
    check_rejected(&target);
}

#[test]
fn vocabulary_and_digests_are_representation_checks_not_backend_availability() {
    let schema: Value = serde_json::from_str(SCHEMA).unwrap();
    for kind in schema["$defs"]["subject"]["properties"]["kind"]["enum"]
        .as_array()
        .unwrap()
    {
        let mut value = base();
        value["subject"]["kind"] = kind.clone();
        assert!(accepts(&value));
    }
    for model in enum_values(&schema, "semantics.integer_model") {
        let mut value = base();
        value["semantics"]["integer_model"] = model;
        assert!(accepts(&value));
    }
    for path in [
        "/schema_version",
        "/subject/kind",
        "/semantics/integer_model",
        "/method",
        "/execution",
        "/result",
        "/scope/kind",
        "/correspondence/relation",
        "/correspondence/status",
    ] {
        let mut value = base();
        *value.pointer_mut(path).unwrap() = json!("FAKE_PRIVATE_UNSUPPORTED");
        check_rejected(&value);
    }
    for path in [
        "/subject/sha256",
        "/specification/sha256",
        "/provenance/configuration_fingerprint",
        "/provenance/bridge_lemma_hash",
    ] {
        for digest in [
            "a".repeat(63),
            "a".repeat(65),
            "A".repeat(64),
            "g".repeat(64),
            format!("{}\n", "a".repeat(63)),
            "FAKE_PRIVATE_DIGEST".to_owned(),
        ] {
            let mut value = base();
            if path.ends_with("bridge_lemma_hash") {
                value["provenance"]["bridge_lemma_hash"] = json!(digest);
            } else {
                *value.pointer_mut(path).unwrap() = json!(digest);
            }
            check_rejected(&value);
        }
    }
}

#[test]
fn string_lists_must_be_unique_nonempty_and_bounds_must_match_scope() {
    for path in [
        "/correspondence/notes",
        "/assumptions",
        "/exclusions",
        "/artifact_paths",
    ] {
        for list in [
            json!([""]),
            json!(["FAKE_PRIVATE_DUPLICATE", "FAKE_PRIVATE_DUPLICATE"]),
            json!([1]),
            json!(null),
        ] {
            let mut value = base();
            *value.pointer_mut(path).unwrap() = list;
            check_rejected(&value);
        }
        for list in [json!([]), json!(["FAKE_PRIVATE_LABEL"])] {
            let mut value = base();
            *value.pointer_mut(path).unwrap() = list;
            assert!(accepts(&value));
        }
    }
    let mut value = base();
    value["scope"] = json!({"kind":"bounded","bounds":["synthetic_limit"]});
    assert!(accepts(&value));
    for bounds in [json!([]), json!([""]), json!(["same", "same"])] {
        value["scope"]["bounds"] = bounds;
        check_rejected(&value);
    }
    value["scope"] = json!({"kind":"unbounded","bounds":["synthetic_limit"]});
    check_rejected(&value);
}

#[test]
fn nonempty_text_optional_translator_and_lemma_values_follow_the_schema() {
    for path in [
        "/subject/id",
        "/specification/id",
        "/obligation_id",
        "/semantics/profile",
        "/provenance/adapter/id",
        "/provenance/adapter/version",
        "/provenance/adapter/protocol_version",
        "/provenance/tool/name",
        "/provenance/tool/version",
    ] {
        let mut value = base();
        *value.pointer_mut(path).unwrap() = json!("");
        check_rejected(&value);
        *value.pointer_mut(path).unwrap() = json!(" ");
        assert!(accepts(&value));
    }
    for translator in [json!(null), json!("synthetic-translator/v1")] {
        for hash in [json!(null), json!("a".repeat(64))] {
            let mut value = base();
            value["provenance"]["translator_version"] = translator.clone();
            value["provenance"]["bridge_lemma_hash"] = hash;
            let evidence = decode(&serde_json::to_vec(&value).unwrap()).unwrap();
            let roundtrip = serde_json::to_value(&evidence).unwrap();
            assert!(accepts(&roundtrip));
            assert_eq!(
                evidence,
                decode(&serde_json::to_vec(&roundtrip).unwrap()).unwrap()
            );
        }
    }
    let mut value = base();
    value["provenance"]["translator_version"] = json!("");
    check_rejected(&value);
}

#[test]
fn malformed_duplicate_non_utf8_and_bounded_inputs_have_only_static_errors() {
    let bytes = serde_json::to_vec(&base()).unwrap();
    let original = String::from_utf8(bytes.clone()).unwrap();
    for (key, duplicate) in [
        (
            "\"method\":",
            "\"method\":\"FAKE_PRIVATE_DUPLICATE\",\"method\":",
        ),
        (
            "\"target_triple\":null",
            "\"target_triple\":null,\"target_triple\":null",
        ),
        (
            "\"method\":",
            "\"\\u006dethod\":\"deductive_proof\",\"method\":",
        ),
    ] {
        let changed = original.replacen(key, duplicate, 1);
        assert_ne!(changed, original);
        assert_eq!(
            decode(changed.as_bytes()).unwrap_err(),
            RepresentationError::InvalidShape
        );
    }
    for malformed in [
        "".into(),
        "FAKE_PRIVATE_JSON".into(),
        "null".into(),
        "[]".into(),
        format!("{original}{original}"),
        original[..original.len() / 2].to_owned(),
        format!("{}0{}", "[".repeat(129), "]".repeat(129)),
    ] {
        assert_eq!(
            decode(malformed.as_bytes()).unwrap_err(),
            RepresentationError::InvalidShape
        );
    }
    assert_eq!(
        decode(&[0xff, 0xfe]).unwrap_err(),
        RepresentationError::InvalidShape
    );
    let mut padded = bytes;
    padded.resize(MAX_INPUT_BYTES, b' ');
    assert!(decode(&padded).is_ok());
    padded.push(b' ');
    assert_eq!(
        decode(&padded).unwrap_err(),
        RepresentationError::InputTooLarge
    );
}

#[test]
fn constructed_values_need_combination_checks_and_diagnostics_do_not_dump_metadata() {
    let mut value = base();
    value["subject"]["id"] = json!("FAKE_PRIVATE_ID");
    value["provenance"]["tool"]["version"] = json!("FAKE_PRIVATE_VERSION");
    let mut evidence = decode(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(!format!("{evidence:?}").contains("FAKE_PRIVATE"));
    evidence.execution = "timeout".into();
    let error = evidence.validate().unwrap_err();
    assert_eq!(error, RepresentationError::InconsistentOutcome);
    assert!(!error.to_string().contains("FAKE_PRIVATE"));
    assert!(!accepts(&serde_json::to_value(&evidence).unwrap()));
    evidence.result = "unknown".into();
    evidence.validate().unwrap();
    assert_eq!(evidence.result, "unknown");
}

#[test]
fn paths_are_untrusted_labels_and_grade_or_command_channels_do_not_exist() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("FAKE_PRIVATE_MUST_NOT_BE_CREATED");
    let mut value = base();
    value["artifact_paths"] = json!([marker.to_str().unwrap(), "../FAKE_PRIVATE_LABEL"]);
    assert!(accepts(&value));
    assert!(!marker.exists());
    for key in [
        "acceptance",
        "grade",
        "proved",
        "artifact_verified",
        "lean_verified",
        "environment",
        "command",
    ] {
        let mut value = base();
        value[key] = json!("FAKE_PRIVATE_PROMOTION_OR_EXECUTION");
        check_rejected(&value);
    }
}

fn list_envelope(elements: Vec<Value>) -> Value {
    json!({"schema_version": LIST_SCHEMA_VERSION, "evidence": elements})
}

#[test]
fn list_envelope_admits_zero_or_more_records_each_through_the_single_gate() {
    for count in [0, 1, 3] {
        let records =
            decode_list(&serde_json::to_vec(&list_envelope(vec![base(); count])).unwrap()).unwrap();
        assert_eq!(records.len(), count);
    }
    // The record cap and byte cap both bound the envelope — whichever trips
    // first, over-limit input is rejected, never silently truncated.
    let oversized = list_envelope(vec![base(); MAX_EVIDENCE_RECORDS + 1]);
    assert!(decode_list(&serde_json::to_vec(&oversized).unwrap()).is_err());
    // A single record is not a list envelope, and vice versa.
    assert!(decode_list(&serde_json::to_vec(&base()).unwrap()).is_err());
    assert!(decode(&serde_json::to_vec(&list_envelope(vec![base()])).unwrap()).is_err());
    // Objects only: a field-ordered JSON sequence is not the closed shape,
    // for the envelope or for a single record.
    assert!(decode_list(br#"["mumei.verification-evidence-list/v1", []]"#).is_err());
    let order = [
        "schema_version",
        "subject",
        "specification",
        "obligation_id",
        "semantics",
        "method",
        "execution",
        "result",
        "scope",
        "correspondence",
        "assumptions",
        "exclusions",
        "provenance",
        "artifact_paths",
    ];
    let record = base();
    let sequence: Vec<Value> = order.iter().map(|key| record[key].clone()).collect();
    let sequence = serde_json::to_vec(&sequence).unwrap();
    assert!(decode(&sequence).is_err());
    let seq_envelope = format!(
        "{{\"schema_version\": \"{LIST_SCHEMA_VERSION}\", \"evidence\": [{}]}}",
        String::from_utf8(sequence).unwrap()
    );
    assert!(decode_list(seq_envelope.as_bytes()).is_err());
    for envelope in [
        json!({"schema_version": SCHEMA_VERSION, "evidence": [base()]}),
        json!({"schema_version": LIST_SCHEMA_VERSION}),
        json!({"schema_version": LIST_SCHEMA_VERSION, "evidence": [], "extra": null}),
        json!({"schema_version": LIST_SCHEMA_VERSION, "evidence": [base(), {"shape": "not-a-record"}]}),
    ] {
        assert!(decode_list(&serde_json::to_vec(&envelope).unwrap()).is_err());
    }
    // A record that would fail standalone fails inside the list too.
    let mut bad = base();
    bad["method"] = json!("FAKE_PRIVATE_METHOD");
    assert!(decode_list(&serde_json::to_vec(&list_envelope(vec![base(), bad])).unwrap()).is_err());
    // Duplicate keys inside an element are still rejected by the inner gate.
    let record = String::from_utf8(serde_json::to_vec(&base()).unwrap()).unwrap();
    let dup = record.replace("\"result\":", "\"execution\": \"completed\",\"result\":");
    let envelope =
        format!("{{\"schema_version\": \"{LIST_SCHEMA_VERSION}\", \"evidence\": [{dup}]}}");
    assert!(decode_list(envelope.as_bytes()).is_err());
}

#[test]
fn consistency_check_compares_identity_pins_never_outcomes() {
    let decode_one = |value: &Value| decode(&serde_json::to_vec(value).unwrap()).unwrap();
    let first = decode_one(&base());

    // Identical records, and records sharing no pin, are consistent.
    let mut independent = base();
    independent["subject"]["id"] = json!("other.mm");
    independent["subject"]["sha256"] = json!("b".repeat(64));
    independent["obligation_id"] = json!("other-obligation");
    assert!(consistency_check(&[first.clone(), first.clone()]).consistent);
    assert!(consistency_check(&[first.clone(), decode_one(&independent)]).consistent);

    // The subject id is a producer label, not an identity: two records with
    // the same `subject.id` but different content pins describe different
    // subjects and stay independent — standalone source records all use
    // "source.mm", so comparing on the id alone would flag them falsely.
    let mut other_subject = base();
    other_subject["subject"]["sha256"] = json!("a".repeat(64));
    other_subject["obligation_id"] = json!("other-obligation");
    assert!(consistency_check(&[first.clone(), decode_one(&other_subject)]).consistent);

    // Same claimed subject (id + sha256) with a different kind is a named
    // conflict.
    let mut reclassified = base();
    reclassified["subject"]["kind"] = json!("mumei_source");
    reclassified["obligation_id"] = json!("other-obligation");
    assert_eq!(
        consistency_check(&[first.clone(), decode_one(&reclassified)]).conflicts,
        vec![ConsistencyConflict {
            index_a: 0,
            index_b: 1,
            field: "subject.kind",
        }]
    );

    // Same obligation_id with rebound pins is a conflict even when the
    // subjects differ — including a different subject content pin.
    let mut rebound = base();
    rebound["subject"]["id"] = json!("other.mm");
    rebound["subject"]["sha256"] = json!("b".repeat(64));
    rebound["specification"]["sha256"] = json!("c".repeat(64));
    let fields: Vec<&str> = consistency_check(&[first.clone(), decode_one(&rebound)])
        .conflicts
        .iter()
        .map(|conflict| conflict.field)
        .collect();
    assert!(fields.contains(&"subject.id"));
    assert!(fields.contains(&"subject.sha256"));
    assert!(fields.contains(&"specification.sha256"));

    // Outcome fields are claims, not identity: differing only there is
    // consistent.
    let mut other = base();
    other["subject"]["id"] = json!("other.mm");
    other["subject"]["sha256"] = json!("b".repeat(64));
    other["obligation_id"] = json!("other-obligation");
    other["result"] = json!("unknown");
    other["execution"] = json!("not_run");
    other["correspondence"]["relation"] = json!("unchecked");
    other["correspondence"]["status"] = json!("unverified");
    assert!(consistency_check(&[first, decode_one(&other)]).consistent);
}

#[test]
fn produced_at_is_optional_and_strictly_rfc3339() {
    let native: Value = serde_json::from_str(NATIVE).unwrap();
    let base = native["evidence"].as_array().unwrap()[0].clone();

    // absent: decodes and serializes with no key — byte-identical shape
    let evidence = decode(&serde_json::to_vec(&base).unwrap()).unwrap();
    assert_eq!(evidence.produced_at, None);
    assert!(serde_json::to_value(&evidence)
        .unwrap()
        .get("produced_at")
        .is_none());

    for stamp in [
        "2026-11-18T00:00:00Z",
        "2026-11-18T00:00:00.9999999Z",
        "2026-11-18t00:00:00z",
        "2026-11-18T00:00:00+09:00",
        "2026-02-31T23:59:60-00:30", // ranges, not day-per-month
    ] {
        let mut value = base.clone();
        value["produced_at"] = json!(stamp);
        let evidence = decode(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(evidence.produced_at.as_deref(), Some(stamp));
        evidence.validate().unwrap();
    }

    for stamp in [
        "not-a-time",
        "2026-11-18",                // date only
        "2026-11-18T00:00Z",         // missing seconds
        "2026-11-18T00:00:00",       // no offset
        "2026-11-18T00:00:00+0900",  // offset colon missing
        "2026-11-18T00:00:00.١Z",    // non-ASCII digit
        "2026-13-18T00:00:00Z",      // month 13
        "2026-11-00T00:00:00Z",      // day 0
        "2026-11-18T24:00:00Z",      // hour 24
        "2026-11-18T00:00:61Z",      // second 61
        "2026-11-18T00:00:00+24:00", // offset hour 24
        "2026-11-18T00:00:00+09:60", // offset minute 60
        "2026-11-18T00:00:00.Z",     // empty fraction
        "",                          // empty
    ] {
        let mut value = base.clone();
        value["produced_at"] = json!(stamp);
        assert_eq!(
            decode(&serde_json::to_vec(&value).unwrap()).unwrap_err(),
            RepresentationError::InvalidValue,
            "stamp: {stamp}"
        );
    }

    // The schema's "type": "string" admits no null: an explicit null is
    // a shape error, never a silent absent field.
    let mut value = base.clone();
    value["produced_at"] = Value::Null;
    assert_eq!(
        decode(&serde_json::to_vec(&value).unwrap()).unwrap_err(),
        RepresentationError::InvalidShape
    );

    // Non-string scalars are shape errors too.
    let mut value = base.clone();
    value["produced_at"] = json!(0);
    assert_eq!(
        decode(&serde_json::to_vec(&value).unwrap()).unwrap_err(),
        RepresentationError::InvalidShape
    );

    // The schema pattern carries the same field ranges — the
    // digit-shaped out-of-range stamp the reviewer tried fails both.
    let mut value = base.clone();
    value["produced_at"] = json!("2026-99-99T99:99:99+99:99");
    assert_eq!(
        decode(&serde_json::to_vec(&value).unwrap()).unwrap_err(),
        RepresentationError::InvalidValue
    );
}

#[test]
fn rfc3339_instants_apply_offsets_and_bound_nanosecond_precision() {
    assert_eq!(rfc3339_instant("1970-01-01T00:00:00Z"), Some((0, 0)));
    assert_eq!(
        rfc3339_instant("1969-12-31T23:59:59Z"),
        Some((-1_000_000_000, -1_000_000_000))
    );
    assert_eq!(
        rfc3339_instant("2024-02-29T09:00:00Z"),
        rfc3339_instant("2024-02-29T18:00:00+09:00")
    );

    let base = rfc3339_instant("2024-02-29T00:00:00Z").unwrap().0;
    let exact = rfc3339_instant("2024-02-29T00:00:00.123456789Z").unwrap();
    assert_eq!(exact, (base + 123_456_789, base + 123_456_789));

    let bounded = rfc3339_instant("2024-02-29T00:00:00.1234567891Z").unwrap();
    assert_eq!(bounded, (base + 123_456_789, base + 123_456_790));
    assert_eq!(
        rfc3339_instant("2024-02-29T00:00:00.123456789000000000000Z"),
        Some((base + 123_456_789, base + 123_456_789))
    );
    assert_eq!(
        rfc3339_instant("2024-02-29T00:00:00.123456789000000000001Z"),
        Some((base + 123_456_789, base + 123_456_790))
    );
}

#[test]
fn rfc3339_instants_reject_impossible_calendar_dates_and_leap_seconds() {
    assert!(rfc3339_instant("2024-02-29T00:00:00Z").is_some());
    assert!(rfc3339_instant("2023-02-29T00:00:00Z").is_none());
    assert!(rfc3339_instant("2024-02-30T00:00:00Z").is_none());
    assert!(rfc3339_instant("2024-02-29T00:00:60Z").is_none());
    assert!(rfc3339_instant("0000-01-01T00:00:00Z").is_none());
}
