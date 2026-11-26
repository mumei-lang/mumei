use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{value::RawValue, Number};
use std::{collections::BTreeMap, fmt};

const MAX_NESTING_DEPTH: usize = 128;

#[derive(Debug)]
pub(crate) enum JsonValue {
    Null,
    Bool(bool),
    Number(JsonNumber),
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

#[derive(Debug)]
pub(crate) enum JsonNumber {
    Integer(Number),
    BigInteger {
        raw: String,
        fallback: Option<Number>,
    },
    Float(Number),
}

impl JsonNumber {
    pub(crate) fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Integer(number) | Self::Float(number) => number.as_u64(),
            Self::BigInteger { fallback, .. } => fallback.as_ref().and_then(Number::as_u64),
        }
    }

    pub(crate) fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Integer(number) | Self::Float(number) => number.as_i64(),
            Self::BigInteger { fallback, .. } => fallback.as_ref().and_then(Number::as_i64),
        }
    }

    pub(crate) fn as_i64_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(number) => number.as_i64(),
            Self::BigInteger { raw, .. } => raw.parse().ok(),
            Self::Float(_) => None,
        }
    }

    pub(crate) fn is_integer_token(&self) -> bool {
        matches!(self, Self::Integer(_) | Self::BigInteger { .. })
    }

    pub(crate) fn is_positive_integer_token(&self) -> bool {
        match self {
            Self::Integer(number) => {
                number.as_i64().is_some_and(|value| value > 0)
                    || number.as_u64().is_some_and(|value| value > 0)
            }
            Self::BigInteger { raw, .. } => {
                !raw.starts_with('-') && raw.bytes().any(|byte| byte != b'0')
            }
            Self::Float(_) => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Error {
    InvalidShape,
    InvalidValue,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidShape => "invalid JSON shape",
            Self::InvalidValue => "invalid JSON value",
        })
    }
}

pub(crate) fn parse(bytes: &[u8]) -> Result<JsonValue, Error> {
    let raw: Box<RawValue> = serde_json::from_slice(bytes).map_err(|_| Error::InvalidShape)?;
    parse_raw_value(raw.get(), 1)
}

pub(crate) fn reject_unrepresentable_integers(value: &JsonValue) -> Result<(), Error> {
    match value {
        JsonValue::Number(JsonNumber::BigInteger { fallback: None, .. }) => {
            Err(Error::InvalidShape)
        }
        JsonValue::Array(values) => {
            for value in values {
                reject_unrepresentable_integers(value)?;
            }
            Ok(())
        }
        JsonValue::Object(values) => {
            for value in values.values() {
                reject_unrepresentable_integers(value)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn parse_raw_value(raw: &str, depth: usize) -> Result<JsonValue, Error> {
    let raw = raw.trim();
    match raw.as_bytes().first().copied() {
        Some(b'-' | b'0'..=b'9') => parse_number(raw),
        Some(_) => {
            let mut deserializer = serde_json::Deserializer::from_str(raw);
            let value = JsonValueSeed { depth }
                .deserialize(&mut deserializer)
                .map_err(|_| Error::InvalidShape)?;
            deserializer.end().map_err(|_| Error::InvalidShape)?;
            Ok(value)
        }
        None => Err(Error::InvalidShape),
    }
}

fn parse_number(raw: &str) -> Result<JsonValue, Error> {
    if raw.bytes().any(|byte| matches!(byte, b'.' | b'e' | b'E')) {
        let number = serde_json::from_str::<Number>(raw).map_err(|_| Error::InvalidShape)?;
        return Ok(JsonValue::Number(JsonNumber::Float(number)));
    }

    let number = serde_json::from_str::<Number>(raw).ok();
    match number {
        Some(number) if number.as_i64().is_some() || number.as_u64().is_some() => {
            Ok(JsonValue::Number(JsonNumber::Integer(number)))
        }
        Some(number) => Ok(JsonValue::Number(JsonNumber::BigInteger {
            raw: raw.to_owned(),
            fallback: Some(number),
        })),
        None => Ok(JsonValue::Number(JsonNumber::BigInteger {
            raw: raw.to_owned(),
            fallback: None,
        })),
    }
}

struct JsonValueSeed {
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for JsonValueSeed {
    type Value = JsonValue;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(JsonValueVisitor { depth: self.depth })
    }
}

struct JsonValueVisitor {
    depth: usize,
}

impl<'de> Visitor<'de> for JsonValueVisitor {
    type Value = JsonValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(JsonValue::Null)
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(JsonValue::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(JsonValue::Number(JsonNumber::Integer(Number::from(value))))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(JsonValue::Number(JsonNumber::Integer(Number::from(value))))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Number::from_f64(value)
            .map(|number| JsonValue::Number(JsonNumber::Float(number)))
            .ok_or_else(|| E::custom("invalid JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(JsonValue::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(JsonValue::String(value))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        if self.depth >= MAX_NESTING_DEPTH {
            return Err(de::Error::custom("JSON nesting limit exceeded"));
        }
        let mut values = Vec::new();
        while let Some(raw) = sequence.next_element::<Box<RawValue>>()? {
            values.push(
                parse_raw_value(raw.get(), self.depth + 1)
                    .map_err(|_| de::Error::custom("invalid JSON value"))?,
            );
        }
        Ok(JsonValue::Array(values))
    }

    fn visit_map<A>(self, mut mapping: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        if self.depth >= MAX_NESTING_DEPTH {
            return Err(de::Error::custom("JSON nesting limit exceeded"));
        }
        let mut values = BTreeMap::new();
        while let Some(key) = mapping.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom("duplicate JSON object key"));
            }
            let raw = mapping.next_value::<Box<RawValue>>()?;
            let value = parse_raw_value(raw.get(), self.depth + 1)
                .map_err(|_| de::Error::custom("invalid JSON value"))?;
            values.insert(key, value);
        }
        Ok(JsonValue::Object(values))
    }
}

pub(crate) fn object(value: &JsonValue) -> Result<&BTreeMap<String, JsonValue>, Error> {
    match value {
        JsonValue::Object(object) => Ok(object),
        _ => Err(Error::InvalidShape),
    }
}

pub(crate) fn require_keys(
    object: &BTreeMap<String, JsonValue>,
    expected: &[&str],
) -> Result<(), Error> {
    if object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key)) {
        Ok(())
    } else {
        Err(Error::InvalidShape)
    }
}

pub(crate) fn field<'a>(
    object: &'a BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<&'a JsonValue, Error> {
    object.get(key).ok_or(Error::InvalidShape)
}

pub(crate) fn string_field<'a>(
    object: &'a BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<&'a str, Error> {
    match field(object, key)? {
        JsonValue::String(value) => Ok(value),
        _ => Err(Error::InvalidValue),
    }
}

pub(crate) fn nullable_string_field<'a>(
    object: &'a BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<Option<&'a str>, Error> {
    match field(object, key)? {
        JsonValue::Null => Ok(None),
        JsonValue::String(value) => Ok(Some(value)),
        _ => Err(Error::InvalidValue),
    }
}

pub(crate) fn unsigned_field(
    object: &BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<u64, Error> {
    match field(object, key)? {
        JsonValue::Number(number) => number.as_u64().ok_or(Error::InvalidValue),
        _ => Err(Error::InvalidValue),
    }
}

pub(crate) fn boolean_field(
    object: &BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<bool, Error> {
    match field(object, key)? {
        JsonValue::Bool(value) => Ok(*value),
        _ => Err(Error::InvalidValue),
    }
}

pub(crate) fn is_integer(value: &JsonValue) -> bool {
    match value {
        JsonValue::Number(number) => number.as_i64().is_some() || number.as_u64().is_some(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn strict_json_rejects_unpaired_surrogate_escapes() {
        assert!(parse(br#"{"opaque":"\ud800"}"#).is_err());
    }

    #[test]
    fn strict_json_matches_serde_json_default_nesting_limit() {
        for depth in 125..=131 {
            let array = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
            let object = format!("{}0{}", r#"{"a":"#.repeat(depth), "}".repeat(depth));
            for (kind, input) in [("array", &array), ("object", &object)] {
                let strict_json_accepts = parse(input.as_bytes()).is_ok();
                let serde_json_accepts =
                    serde_json::from_slice::<serde_json::Value>(input.as_bytes()).is_ok();
                assert_eq!(
                    strict_json_accepts, serde_json_accepts,
                    "{kind} nesting depth {depth}"
                );
            }
        }
    }
}
