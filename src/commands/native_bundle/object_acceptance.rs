//! Closed decoding of a saved `accept-object --json` descriptor for the
//! consumer-side replay in `recheck-object-acceptance`.

use mumei_core::native_bundle::{is_digest, BundleError};
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::fmt;

const SCHEMA_VERSION: &str = "mumei.native-evidence-acceptance/v1";
pub(super) const SCOPES: [&str; 2] = [
    "native_bundle_object_translation_correspondence",
    "native_bundle_object_translation_correspondence_without_object_link",
];

/// A JSON value whose objects never repeat a key: `serde_json::Value`
/// keeps the last duplicate silently, which would let two readers of the
/// same descriptor disagree on what it claims.
struct UniqueKeys(Value);

impl<'de> Deserialize<'de> for UniqueKeys {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer
            .deserialize_any(UniqueKeysVisitor)
            .map(UniqueKeys)
    }
}

struct UniqueKeysVisitor;

impl<'de> Visitor<'de> for UniqueKeysVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_str<E>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(UniqueKeys(value)) = seq.next_element()? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let UniqueKeys(value) = map.next_value()?;
            if object.insert(key, value).is_some() {
                return Err(de::Error::custom("duplicate object key"));
            }
        }
        Ok(Value::Object(object))
    }
}

/// Decodes a saved object acceptance descriptor. Only the envelope is
/// validated here; every other field is bound by equality with the fresh
/// descriptor, so a field this decoder does not name cannot slip through.
pub(super) fn decode(bytes: &[u8]) -> Result<Value, BundleError> {
    const MALFORMED: BundleError = BundleError("malformed native object acceptance descriptor");
    let UniqueKeys(value) = serde_json::from_slice(bytes).map_err(|_| MALFORMED)?;
    let envelope_valid = value["schema_version"] == SCHEMA_VERSION
        && value["acceptance"] == "evaluated"
        && value["accepted"] == true
        && value["scope"]
            .as_str()
            .is_some_and(|scope| SCOPES.contains(&scope))
        && value["evidence_fingerprint"]
            .as_str()
            .is_some_and(is_digest);
    if envelope_valid {
        Ok(value)
    } else {
        Err(MALFORMED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope() -> Value {
        serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "acceptance": "evaluated",
            "accepted": true,
            "scope": SCOPES[0],
            "evidence_fingerprint": "a".repeat(64),
        })
    }

    #[test]
    fn decode_accepts_object_acceptance_envelope() {
        let bytes = serde_json::to_vec(&envelope()).unwrap();
        assert_eq!(decode(&bytes).unwrap(), envelope());
    }

    #[test]
    fn decode_rejects_duplicate_keys_at_any_depth() {
        let top = br#"{"accepted":true,"accepted":true}"#;
        assert!(decode(top).is_err());
        let mut nested = serde_json::to_string(&envelope()).unwrap();
        nested.pop();
        nested.push_str(r#","functions":{"main":{"outcome":"a","outcome":"b"}}}"#);
        assert!(serde_json::from_str::<Value>(&nested).is_ok());
        assert!(decode(nested.as_bytes()).is_err());
    }

    #[test]
    fn decode_rejects_foreign_or_unaccepted_envelopes() {
        for (key, value) in [
            (
                "schema_version",
                Value::from("mumei.native-consumer-evidence/v1"),
            ),
            ("acceptance", Value::from("not_evaluated")),
            ("accepted", Value::from(false)),
            (
                "scope",
                Value::from("native_bundle_identity_source_llvm_and_saved_evidence_consistency"),
            ),
            ("evidence_fingerprint", Value::from("A".repeat(64))),
            ("evidence_fingerprint", Value::Null),
        ] {
            let mut descriptor = envelope();
            descriptor[key] = value;
            let bytes = serde_json::to_vec(&descriptor).unwrap();
            assert!(decode(&bytes).is_err(), "{key}");
        }
        assert!(decode(b"[]").is_err());
        assert!(decode(b"{").is_err());
        assert!(decode(&[0xff]).is_err());
    }
}
