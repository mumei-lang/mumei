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
    StringWithSurrogate,
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
    parse_inner(bytes, false, false)
}

pub(crate) fn parse_with_surrogate_strings(bytes: &[u8]) -> Result<JsonValue, Error> {
    parse_inner(bytes, true, true)
}

fn parse_inner(
    bytes: &[u8],
    allow_surrogate_strings: bool,
    allow_big_integers: bool,
) -> Result<JsonValue, Error> {
    let (normalized, surrogate_placeholder) = normalize_surrogate_escapes(bytes)?;
    if surrogate_placeholder.is_some() && !allow_surrogate_strings {
        return Err(Error::InvalidShape);
    }
    let raw: Box<RawValue> =
        serde_json::from_slice(&normalized).map_err(|_| Error::InvalidShape)?;
    parse_raw_value(raw.get(), 0, surrogate_placeholder, allow_big_integers)
}

fn normalize_surrogate_escapes(bytes: &[u8]) -> Result<(Vec<u8>, Option<char>), Error> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::InvalidShape)?;
    let placeholder = surrogate_placeholder(text);
    let source = text.as_bytes();
    let mut normalized = Vec::with_capacity(source.len());
    let mut in_string = false;
    let mut replaced = false;
    let mut index = 0;
    while index < source.len() {
        let byte = source[index];
        if !in_string {
            if byte == b'"' {
                in_string = true;
            }
            normalized.push(byte);
            index += 1;
            continue;
        }
        if byte == b'"' {
            in_string = false;
            normalized.push(byte);
            index += 1;
            continue;
        }
        if byte != b'\\' || source.get(index + 1) != Some(&b'u') {
            normalized.push(byte);
            index += 1;
            if byte == b'\\' && index < source.len() {
                normalized.push(source[index]);
                index += 1;
            }
            continue;
        }

        let Some(unit) = unicode_escape(source, index) else {
            normalized.extend_from_slice(&source[index..(index + 2).min(source.len())]);
            index += 2;
            continue;
        };
        if (0xd800..=0xdbff).contains(&unit) {
            if let Some(low) =
                unicode_escape(source, index + 6).filter(|low| (0xdc00..=0xdfff).contains(low))
            {
                normalized.extend_from_slice(&source[index..index + 12]);
                let scalar =
                    0x10000 + ((u32::from(unit) - 0xd800) << 10) + (u32::from(low) - 0xdc00);
                if char::from_u32(scalar) == placeholder {
                    return Err(Error::InvalidShape);
                }
                index += 12;
                continue;
            }
            let placeholder = placeholder.ok_or(Error::InvalidShape)?;
            append_placeholder(&mut normalized, placeholder);
            replaced = true;
            index += 6;
            continue;
        }
        if (0xdc00..=0xdfff).contains(&unit) {
            let placeholder = placeholder.ok_or(Error::InvalidShape)?;
            append_placeholder(&mut normalized, placeholder);
            replaced = true;
            index += 6;
            continue;
        }
        normalized.extend_from_slice(&source[index..index + 6]);
        index += 6;
    }
    Ok((normalized, if replaced { placeholder } else { None }))
}

fn unicode_escape(bytes: &[u8], index: usize) -> Option<u16> {
    let escape = bytes.get(index..index + 6)?;
    if escape[0] != b'\\' || escape[1] != b'u' {
        return None;
    }
    let mut value = 0u16;
    for digit in &escape[2..] {
        value = (value << 4)
            | match digit {
                b'0'..=b'9' => u16::from(*digit - b'0'),
                b'a'..=b'f' => u16::from(*digit - b'a' + 10),
                b'A'..=b'F' => u16::from(*digit - b'A' + 10),
                _ => return None,
            };
    }
    Some(value)
}

fn surrogate_placeholder(text: &str) -> Option<char> {
    let mut used = std::collections::HashSet::new();
    for character in text.chars() {
        used.insert(character);
    }
    let bytes = text.as_bytes();
    for index in 0..bytes.len().saturating_sub(5) {
        if let Some(unit) = unicode_escape(bytes, index) {
            if let Some(character) = char::from_u32(u32::from(unit)) {
                used.insert(character);
            }
            if (0xd800..=0xdbff).contains(&unit) {
                if let Some(low) =
                    unicode_escape(bytes, index + 6).filter(|low| (0xdc00..=0xdfff).contains(low))
                {
                    let scalar =
                        0x10000 + ((u32::from(unit) - 0xd800) << 10) + (u32::from(low) - 0xdc00);
                    if let Some(character) = char::from_u32(scalar) {
                        used.insert(character);
                    }
                }
            }
        }
    }
    (1..=0xd7ff)
        .chain(0xe000..=0x10ffff)
        .filter_map(char::from_u32)
        .find(|character| !used.contains(character))
}

fn append_placeholder(output: &mut Vec<u8>, placeholder: char) {
    let mut units = [0; 2];
    for unit in placeholder.encode_utf16(&mut units) {
        output.extend_from_slice(format!("\\u{unit:04X}").as_bytes());
    }
}

fn parse_raw_value(
    raw: &str,
    depth: usize,
    surrogate_placeholder: Option<char>,
    allow_big_integers: bool,
) -> Result<JsonValue, Error> {
    let raw = raw.trim();
    match raw.as_bytes().first().copied() {
        Some(b'-' | b'0'..=b'9') => parse_number(raw, allow_big_integers),
        Some(_) => {
            let mut deserializer = serde_json::Deserializer::from_str(raw);
            let value = JsonValueSeed {
                depth,
                surrogate_placeholder,
                allow_big_integers,
            }
            .deserialize(&mut deserializer)
            .map_err(|_| Error::InvalidShape)?;
            deserializer.end().map_err(|_| Error::InvalidShape)?;
            Ok(value)
        }
        None => Err(Error::InvalidShape),
    }
}

fn parse_number(raw: &str, allow_big_integers: bool) -> Result<JsonValue, Error> {
    if raw.bytes().any(|byte| matches!(byte, b'.' | b'e' | b'E')) {
        let number = serde_json::from_str::<Number>(raw).map_err(|_| Error::InvalidShape)?;
        return Ok(JsonValue::Number(JsonNumber::Float(number)));
    }

    let number = serde_json::from_str::<Number>(raw).ok();
    match number {
        Some(number) if number.as_i64().is_some() || number.as_u64().is_some() => {
            Ok(JsonValue::Number(JsonNumber::Integer(number)))
        }
        Some(number) if allow_big_integers => Ok(JsonValue::Number(JsonNumber::BigInteger {
            raw: raw.to_owned(),
            fallback: Some(number),
        })),
        Some(number) => Ok(JsonValue::Number(JsonNumber::Float(number))),
        None if allow_big_integers => Ok(JsonValue::Number(JsonNumber::BigInteger {
            raw: raw.to_owned(),
            fallback: None,
        })),
        None => Err(Error::InvalidShape),
    }
}

struct JsonValueSeed {
    depth: usize,
    surrogate_placeholder: Option<char>,
    allow_big_integers: bool,
}

impl<'de> DeserializeSeed<'de> for JsonValueSeed {
    type Value = JsonValue;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(JsonValueVisitor {
            depth: self.depth,
            surrogate_placeholder: self.surrogate_placeholder,
            allow_big_integers: self.allow_big_integers,
        })
    }
}

struct JsonValueVisitor {
    depth: usize,
    surrogate_placeholder: Option<char>,
    allow_big_integers: bool,
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
        if self
            .surrogate_placeholder
            .is_some_and(|placeholder| value.contains(placeholder))
        {
            Ok(JsonValue::StringWithSurrogate)
        } else {
            Ok(JsonValue::String(value.to_owned()))
        }
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if self
            .surrogate_placeholder
            .is_some_and(|placeholder| value.contains(placeholder))
        {
            Ok(JsonValue::StringWithSurrogate)
        } else {
            Ok(JsonValue::String(value))
        }
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
                parse_raw_value(
                    raw.get(),
                    self.depth + 1,
                    self.surrogate_placeholder,
                    self.allow_big_integers,
                )
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
            let value = parse_raw_value(
                raw.get(),
                self.depth + 1,
                self.surrogate_placeholder,
                self.allow_big_integers,
            )
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
    fn attempt_schedule_parser_rejects_unpaired_surrogate_escapes() {
        assert!(parse(br#"{"opaque":"\ud800"}"#).is_err());
    }
}
