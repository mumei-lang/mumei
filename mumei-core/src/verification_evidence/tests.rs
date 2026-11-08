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
