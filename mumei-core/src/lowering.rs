//! Canonical lowering of Mumei surface type names.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoweredType {
    I64,
    I32,
    U64,
    U32,
    F64,
    F32,
    Bool,
    Str,
    Array(Box<LoweredType>),
    Other(String),
}

pub fn lower(type_name: &str) -> LoweredType {
    let n = type_name.trim();
    if n.starts_with("[]<") && n.ends_with('>') {
        return LoweredType::Array(Box::new(lower(&n[3..n.len() - 1])));
    }
    if n.starts_with('[') && n.ends_with(']') {
        return LoweredType::Array(Box::new(lower(n[1..n.len() - 1].trim())));
    }
    match n {
        "i64" => LoweredType::I64,
        "i32" => LoweredType::I32,
        "u64" => LoweredType::U64,
        "u32" => LoweredType::U32,
        "f64" => LoweredType::F64,
        "f32" => LoweredType::F32,
        "bool" => LoweredType::Bool,
        "Str" | "String" => LoweredType::Str,
        other => LoweredType::Other(other.to_string()),
    }
}

/// Extract `T` from a declared `chan<T>` type name.
pub fn chan_payload_type(type_name: &str) -> Option<String> {
    let inner = type_name
        .trim()
        .strip_prefix("chan")?
        .trim_start()
        .strip_prefix('<')?
        .strip_suffix('>')?
        .trim();
    if inner.is_empty() {
        None
    } else {
        Some(inner.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{chan_payload_type, lower, LoweredType};

    #[test]
    fn test_lower_exact_tokens() {
        assert_eq!(lower("i64"), LoweredType::I64);
        assert_eq!(lower("i32"), LoweredType::I32);
        assert_eq!(lower("u64"), LoweredType::U64);
        assert_eq!(lower("u32"), LoweredType::U32);
        assert_eq!(lower("f64"), LoweredType::F64);
        assert_eq!(lower("f32"), LoweredType::F32);
        assert_eq!(lower("bool"), LoweredType::Bool);
        assert_eq!(lower("Str"), LoweredType::Str);
        assert_eq!(lower("String"), LoweredType::Str);
    }

    #[test]
    fn test_lower_arrays() {
        assert_eq!(
            lower("[i64]"),
            LoweredType::Array(Box::new(LoweredType::I64))
        );
        assert_eq!(
            lower("[f64]"),
            LoweredType::Array(Box::new(LoweredType::F64))
        );
        assert_eq!(
            lower("[]<i64>"),
            LoweredType::Array(Box::new(LoweredType::I64))
        );
        assert_eq!(
            lower("[[i64]]"),
            LoweredType::Array(Box::new(LoweredType::Array(Box::new(LoweredType::I64))))
        );
    }

    #[test]
    fn test_chan_payload_type() {
        assert_eq!(chan_payload_type("chan<f64>"), Some("f64".to_string()));
        assert_eq!(chan_payload_type("chan <Str>"), Some("Str".to_string()));
        assert_eq!(chan_payload_type("chan<[i64]>"), Some("[i64]".to_string()));
        assert_eq!(chan_payload_type("chan<>"), None);
        assert_eq!(chan_payload_type("chan"), None);
        assert_eq!(chan_payload_type("i64"), None);
        assert_eq!(chan_payload_type("channel<i64>"), None);
    }

    #[test]
    fn test_lower_unknown() {
        assert_eq!(
            lower("UnknownType"),
            LoweredType::Other("UnknownType".to_string())
        );
    }
}
