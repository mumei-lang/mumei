use super::module_env::ModuleEnv;
use super::translator::struct_field_key;
use crate::lowering::{lower, LoweredType};
use crate::parser::{Atom, EnumDef};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use z3::ast::{Ast, Dynamic, Int};
use z3::Model;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RaisedStatus {
    Raised,
    Unraisable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaisedValue {
    pub source_name: String,
    pub solver_name: Option<String>,
    pub rendering: String,
    pub lowering: &'static str,
    pub source_type: Option<String>,
    pub status: RaisedStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RaisedCounterexample {
    pub values: Vec<RaisedValue>,
    pub omitted_solver_symbols: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SolverSymbolClassification {
    Source(String),
    TupleComponent(String),
    OldState(String),
    Internal,
}

impl RaisedCounterexample {
    pub fn to_counterexample_json(&self) -> Value {
        let mut values = Map::new();
        for value in &self.values {
            values.insert(value.source_name.clone(), json!(value.rendering));
        }
        Value::Object(values)
    }

    pub fn provenance_json(&self) -> Value {
        let mut values = Map::new();
        for value in &self.values {
            let mut entry = Map::new();
            match &value.status {
                RaisedStatus::Raised => {
                    entry.insert("status".to_string(), json!("raised"));
                }
                RaisedStatus::Unraisable { reason } => {
                    entry.insert("status".to_string(), json!("unraisable"));
                    entry.insert("reason".to_string(), json!(reason));
                }
            }
            entry.insert("lowering".to_string(), json!(value.lowering));
            if let Some(source_type) = &value.source_type {
                entry.insert("source_type".to_string(), json!(source_type));
            }
            if let Some(solver_name) = &value.solver_name {
                entry.insert("solver_name".to_string(), json!(solver_name));
            }
            values.insert(value.source_name.clone(), Value::Object(entry));
        }
        json!({
            "complete": self.is_complete(),
            "values": Value::Object(values),
            "omitted_solver_symbols": self.omitted_solver_symbols,
        })
    }

    pub fn is_complete(&self) -> bool {
        self.values
            .iter()
            .all(|value| matches!(&value.status, RaisedStatus::Raised))
    }

    pub fn from_bindings<'ctx>(
        model: &Model<'ctx>,
        bindings: impl IntoIterator<Item = (String, Option<String>, Dynamic<'ctx>)>,
        env: &HashMap<String, Dynamic<'ctx>>,
        module_env: &ModuleEnv,
    ) -> Self {
        let bindings: Vec<_> = bindings.into_iter().collect();
        let mut raised = Self::default();
        let mut selected = HashSet::new();
        for (name, source_type, value) in &bindings {
            selected.insert(name.clone());
            let evaluated = model.eval(value, true).unwrap_or_else(|| value.clone());
            let (rendering, lowering, status) = raise_model_value_with_env(
                model,
                &evaluated,
                source_type.as_deref(),
                module_env,
                env,
                Some(name),
            );
            raised.values.push(RaisedValue {
                source_name: name.clone(),
                solver_name: None,
                rendering,
                lowering,
                source_type: source_type.clone(),
                status,
            });
        }
        for symbol in env.keys() {
            if selected.contains(symbol) {
                continue;
            }
            if helper_symbol_is_for_binding(symbol, &selected, module_env) {
                raised.omitted_solver_symbols.push(symbol.clone());
                continue;
            }
            match classify_solver_symbol(symbol, &selected, module_env) {
                SolverSymbolClassification::TupleComponent(source_name) => {
                    let tuple_binding = source_name.rsplit_once("._").map(|(binding, _)| binding);
                    if tuple_binding
                        .is_some_and(|binding| selected.contains(binding) || binding == "result")
                    {
                        let rendering = model
                            .eval(env.get(symbol).expect("symbol came from env"), true)
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| symbol.clone());
                        raised.values.push(RaisedValue {
                            source_name,
                            solver_name: Some(symbol.clone()),
                            rendering,
                            lowering: "tuple",
                            source_type: None,
                            status: RaisedStatus::Unraisable {
                                reason: "tuple result components are not linked to the body"
                                    .to_string(),
                            },
                        });
                    } else {
                        raised.omitted_solver_symbols.push(symbol.clone());
                    }
                }
                SolverSymbolClassification::OldState(source_name) => {
                    let value = env.get(symbol).expect("symbol came from env");
                    let rendering = model
                        .eval(value, true)
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| value.to_string());
                    raised.values.push(RaisedValue {
                        source_name,
                        solver_name: Some(symbol.clone()),
                        rendering,
                        lowering: "old",
                        source_type: None,
                        status: RaisedStatus::Unraisable {
                            reason: "pre-state provenance is unavailable".to_string(),
                        },
                    });
                }
                SolverSymbolClassification::Internal => {
                    raised.omitted_solver_symbols.push(symbol.clone());
                }
                SolverSymbolClassification::Source(_) => {
                    raised.omitted_solver_symbols.push(symbol.clone());
                }
            }
        }
        raised
            .values
            .sort_by(|a, b| a.source_name.cmp(&b.source_name));
        raised.omitted_solver_symbols.sort();
        raised.omitted_solver_symbols.dedup();
        raised
    }
}

pub fn classify_solver_symbol(
    name: &str,
    source_bindings: &HashSet<String>,
    module_env: &ModuleEnv,
) -> SolverSymbolClassification {
    if let Some(rest) = name.strip_prefix("__mumei_tuple_result_") {
        if let Some((binding, suffix)) = rest.rsplit_once('_') {
            if let Ok(index) = suffix.parse::<usize>() {
                if source_bindings.contains(binding) || binding == "result" {
                    return SolverSymbolClassification::TupleComponent(format!(
                        "{binding}._{index}"
                    ));
                }
            }
        }
        return SolverSymbolClassification::Internal;
    }
    if let Some(binding) = name.strip_prefix("__old_") {
        return SolverSymbolClassification::OldState(format!("old({binding})"));
    }
    for binding in source_bindings {
        if let Some(field) = name
            .strip_prefix(&format!("__struct_{binding}_"))
            .or_else(|| name.strip_prefix(&format!("{binding}_")))
        {
            if module_env
                .structs
                .values()
                .any(|structure| structure.fields.iter().any(|entry| entry.name == field))
            {
                return SolverSymbolClassification::Source(format!("{binding}.{field}"));
            }
        }
    }
    if is_internal_solver_symbol(name) {
        SolverSymbolClassification::Internal
    } else {
        SolverSymbolClassification::Source(name.to_string())
    }
}

pub fn raise_model_value<'ctx>(
    model: &Model<'ctx>,
    value: &Dynamic<'ctx>,
    type_hint: Option<&str>,
    module_env: &ModuleEnv,
) -> (String, &'static str, RaisedStatus) {
    raise_model_value_with_env(model, value, type_hint, module_env, &HashMap::new(), None)
}

pub fn raise_named_model_value<'ctx>(
    model: &Model<'ctx>,
    source_name: impl Into<String>,
    value: &Dynamic<'ctx>,
    type_hint: Option<&str>,
    module_env: &ModuleEnv,
) -> RaisedValue {
    let (rendering, lowering, status) = raise_model_value(model, value, type_hint, module_env);
    RaisedValue {
        source_name: source_name.into(),
        solver_name: None,
        rendering,
        lowering,
        source_type: type_hint.map(str::to_string),
        status,
    }
}

pub fn raise_binding<'ctx>(
    model: &Model<'ctx>,
    binding: &str,
    value: &Dynamic<'ctx>,
    type_hint: Option<&str>,
    env: &HashMap<String, Dynamic<'ctx>>,
    module_env: &ModuleEnv,
) -> (String, &'static str, RaisedStatus) {
    let evaluated = model.eval(value, true).unwrap_or_else(|| value.clone());
    raise_model_value_with_env(model, &evaluated, type_hint, module_env, env, Some(binding))
}

fn raise_model_value_with_env<'ctx>(
    model: &Model<'ctx>,
    value: &Dynamic<'ctx>,
    type_hint: Option<&str>,
    module_env: &ModuleEnv,
    env: &HashMap<String, Dynamic<'ctx>>,
    binding: Option<&str>,
) -> (String, &'static str, RaisedStatus) {
    let raw = value.to_string();
    let source_base = type_hint.map(|name| resolve_source_base_type(name, module_env));
    let base = source_base.as_deref().or_else(|| {
        if value.as_bv().is_some() {
            Some("i64")
        } else if value.as_float().is_some() || value.as_real().is_some() {
            Some("f64")
        } else if value.as_bool().is_some() {
            Some("bool")
        } else if value.as_string().is_some() {
            Some("Str")
        } else {
            None
        }
    });

    if let Some(type_name) = base {
        if let Some(enum_def) = module_env.get_enum(type_name) {
            return raise_enum_value(model, value, enum_def, module_env, env, binding);
        }
        if let Some(enum_def) = module_env.find_enum_by_variant(type_name) {
            return raise_enum_value(model, value, enum_def, module_env, env, binding);
        }
        if module_env.get_struct(type_name).is_some() {
            let Some(binding) = binding else {
                return unraisable(raw, "struct", "struct values require their binding fields");
            };
            return raise_struct_value(model, binding, value, type_name, env, module_env);
        }
        if matches!(lower(type_name), LoweredType::Array(_)) {
            let Some(binding) = binding else {
                return unraisable(
                    raw,
                    "array",
                    "array value has no source binding for its length",
                );
            };
            return raise_array_value(model, value, type_name, binding, env, module_env);
        }
    }

    match base.map(lower) {
        Some(LoweredType::I64) if value.as_bv().is_some() => {
            let bv = value.as_bv().expect("checked above");
            if bv.get_size() != 64 {
                return unraisable(raw, "bitvec_i64", "expected a 64-bit bit-vector");
            }
            bv.as_u64()
                .map(|bits| (bits as i64).to_string())
                .or_else(|| bv.as_i64().map(|integer| integer.to_string()))
                .map(|rendering| (rendering, "bitvec_i64", RaisedStatus::Raised))
                .unwrap_or_else(|| {
                    unraisable(
                        raw.clone(),
                        "bitvec_i64",
                        "could not decode the 64-bit value",
                    )
                })
        }
        Some(LoweredType::I64) | None if value.as_int().is_some() => value
            .as_int()
            .and_then(|integer| integer.as_i64())
            .map(|integer| (integer.to_string(), "int", RaisedStatus::Raised))
            .or_else(|| {
                parse_solver_integer(&raw).map(|integer| (integer, "int", RaisedStatus::Raised))
            })
            .unwrap_or_else(|| {
                unraisable(raw.clone(), "int", "integer is outside the source range")
            }),
        Some(LoweredType::F64) if value.as_float().is_some() => decode_ieee754_f64(&raw)
            .map(|number| (format!("{number:?}"), "ieee754_f64", RaisedStatus::Raised))
            .unwrap_or_else(|| {
                unraisable(
                    raw.clone(),
                    "ieee754_f64",
                    "unsupported IEEE 754 model rendering",
                )
            }),
        Some(LoweredType::F64) if value.as_real().is_some() => {
            let exact = value
                .as_real()
                .and_then(|real| real.as_real())
                .and_then(|(numerator, denominator)| exact_dyadic_f64(numerator, denominator));
            exact
                .map(|number| (format!("{number:?}"), "real_f64", RaisedStatus::Raised))
                .unwrap_or_else(|| unraisable(raw.clone(), "real_f64", "no exact f64 rendering"))
        }
        Some(LoweredType::Bool) => value
            .as_bool()
            .and_then(|boolean| boolean.as_bool())
            .map(|boolean| (boolean.to_string(), "bool", RaisedStatus::Raised))
            .unwrap_or_else(|| {
                unraisable(raw.clone(), "bool", "could not decode the boolean value")
            }),
        Some(LoweredType::Str) => (raw, "string", RaisedStatus::Raised),
        _ => unraisable(
            raw,
            "unknown",
            "no source lowering is available for this model value",
        ),
    }
}

fn raise_enum_value<'ctx>(
    model: &Model<'ctx>,
    value: &Dynamic<'ctx>,
    enum_def: &EnumDef,
    module_env: &ModuleEnv,
    env: &HashMap<String, Dynamic<'ctx>>,
    _binding: Option<&str>,
) -> (String, &'static str, RaisedStatus) {
    let raw = value.to_string();
    let (variant_index, args) = if let Some(datatype) = value.as_datatype() {
        let variant_name = datatype.decl().name().to_string();
        let Some(index) = enum_def
            .variants
            .iter()
            .position(|variant| variant.name == variant_name)
        else {
            return unraisable(
                raw,
                "enum",
                "datatype constructor is absent from enum metadata",
            );
        };
        (index, datatype.children())
    } else if let Some(integer) = value.as_int().and_then(|integer| integer.as_i64()) {
        let Ok(index) = usize::try_from(integer) else {
            return unraisable(raw, "enum", "enum tag is outside the declared variants");
        };
        let Some(variant) = enum_def.variants.get(index) else {
            return unraisable(raw, "enum", "enum tag is outside the declared variants");
        };
        let args = variant
            .fields
            .iter()
            .enumerate()
            .map(|(field_index, _)| {
                let enum_projection =
                    format!("__proj_{}_{}_{}", enum_def.name, variant.name, field_index);
                let variant_projection = format!("__proj_{}_{}", variant.name, field_index);
                env.get(&enum_projection)
                    .or_else(|| env.get(&variant_projection))
                    .and_then(|field| model.eval(field, true))
            })
            .collect::<Option<Vec<_>>>();
        let Some(args) = args else {
            if variant.fields.is_empty() {
                return (
                    format!("{}::{}", enum_def.name, variant.name),
                    "enum",
                    RaisedStatus::Raised,
                );
            }
            return unraisable(raw, "enum", "enum payload projection is unavailable");
        };
        (index, args)
    } else {
        return unraisable(
            raw,
            "enum",
            "model value is not a recognized enum constructor or tag",
        );
    };
    let variant = &enum_def.variants[variant_index];
    if args.len() != variant.fields.len() {
        if variant.fields.is_empty() {
            return (
                format!("{}::{}", enum_def.name, variant.name),
                "enum",
                RaisedStatus::Raised,
            );
        }
        return unraisable(
            raw,
            "enum",
            "enum payload arity does not match its declaration",
        );
    }
    let fields = args
        .iter()
        .zip(&variant.field_types)
        .map(|(arg, field_type)| {
            let (rendering, _, status) = raise_model_value_with_env(
                model,
                arg,
                Some(&field_type.display_name()),
                module_env,
                env,
                None,
            );
            if matches!(status, RaisedStatus::Raised) {
                Some(rendering)
            } else {
                None
            }
        })
        .collect::<Option<Vec<_>>>();
    let Some(fields) = fields else {
        return unraisable(raw, "enum", "one or more enum payloads could not be raised");
    };
    if fields.is_empty() {
        (
            format!("{}::{}", enum_def.name, variant.name),
            "enum",
            RaisedStatus::Raised,
        )
    } else {
        (
            format!("{}::{}({})", enum_def.name, variant.name, fields.join(", ")),
            "enum",
            RaisedStatus::Raised,
        )
    }
}

fn raise_struct_value<'ctx>(
    model: &Model<'ctx>,
    binding: &str,
    value: &Dynamic<'ctx>,
    type_name: &str,
    env: &HashMap<String, Dynamic<'ctx>>,
    module_env: &ModuleEnv,
) -> (String, &'static str, RaisedStatus) {
    let raw = value.to_string();
    let Some(struct_def) = module_env.get_struct(type_name) else {
        return unraisable(raw, "struct", "struct metadata is unavailable");
    };
    let mut fields = Vec::new();
    let flat_binding = binding.replace('.', "_");
    for field in &struct_def.fields {
        let key = struct_field_key(&flat_binding, &field.name);
        let alternate_key = struct_field_key(binding, &field.name);
        let Some(field_value) = env.get(&key).or_else(|| env.get(&alternate_key)) else {
            return unraisable(
                raw,
                "struct",
                &format!("missing struct field `{}`", field.name),
            );
        };
        let evaluated = model
            .eval(field_value, true)
            .unwrap_or_else(|| field_value.clone());
        let (rendering, _, status) = raise_model_value_with_env(
            model,
            &evaluated,
            Some(&field.type_name),
            module_env,
            env,
            Some(&format!("{binding}.{}", field.name)),
        );
        if let RaisedStatus::Unraisable { reason } = status {
            return unraisable(
                raw,
                "struct",
                &format!("field `{}` could not be raised: {reason}", field.name),
            );
        }
        fields.push(format!("{}: {}", field.name, rendering));
    }
    (
        format!("{} {{ {} }}", struct_def.name, fields.join(", ")),
        "struct",
        RaisedStatus::Raised,
    )
}

fn raise_array_value<'ctx>(
    model: &Model<'ctx>,
    value: &Dynamic<'ctx>,
    type_name: &str,
    binding: &str,
    env: &HashMap<String, Dynamic<'ctx>>,
    module_env: &ModuleEnv,
) -> (String, &'static str, RaisedStatus) {
    let raw = value.to_string();
    let Some(len_value) = env.get(&format!("len_{binding}")) else {
        return unraisable(raw, "array", "array length is unavailable");
    };
    let Some(length) = model
        .eval(len_value, true)
        .and_then(|length| integer_value(&length))
    else {
        return unraisable(raw, "array", "array length is unknown");
    };
    if !(0..=32).contains(&length) {
        return unraisable(
            raw,
            "array",
            "array length is outside the supported range 0..=32",
        );
    }
    let Some(array) = value.as_array() else {
        return unraisable(raw, "array", "model value is not an array");
    };
    let element_type = type_name
        .strip_prefix('[')
        .and_then(|name| name.strip_suffix(']'))
        .or_else(|| {
            type_name
                .strip_prefix("[]<")
                .and_then(|name| name.strip_suffix('>'))
        })
        .unwrap_or("i64");
    let mut elements = Vec::new();
    for index in 0..length {
        let selected = model
            .eval(&array.select(&Int::from_i64(value.get_ctx(), index)), true)
            .unwrap_or_else(|| array.select(&Int::from_i64(value.get_ctx(), index)));
        let (rendering, _, status) =
            raise_model_value_with_env(model, &selected, Some(element_type), module_env, env, None);
        if let RaisedStatus::Unraisable { reason } = status {
            return unraisable(
                raw,
                "array",
                &format!("array element {index} could not be raised: {reason}"),
            );
        }
        elements.push(rendering);
    }
    (
        format!("[{}]", elements.join(", ")),
        "array",
        RaisedStatus::Raised,
    )
}

fn exact_dyadic_f64(numerator: i64, denominator: i64) -> Option<f64> {
    if denominator == 0 {
        return None;
    }
    let numerator = i128::from(numerator);
    let denominator = i128::from(denominator);
    if numerator.abs() > (1i128 << 53) || !denominator.unsigned_abs().is_power_of_two() {
        return None;
    }
    let value = numerator as f64 / denominator as f64;
    if !value.is_finite() {
        return None;
    }
    let scaled = value * denominator as f64;
    (scaled == numerator as f64).then_some(value)
}

fn resolve_source_base_type(type_name: &str, module_env: &ModuleEnv) -> String {
    let mut base = type_name.to_string();
    let mut seen = HashSet::new();
    while seen.insert(base.clone()) {
        let Some(refined_type) = module_env.get_type(&base) else {
            break;
        };
        if refined_type._base_type == base {
            break;
        }
        base = refined_type._base_type.clone();
    }
    base
}

fn parse_solver_integer(raw: &str) -> Option<String> {
    if let Some(inner) = raw
        .strip_prefix("(- ")
        .and_then(|value| value.strip_suffix(')'))
    {
        let integer = inner.parse::<i128>().ok()?;
        Some((-integer).to_string())
    } else {
        raw.parse::<i128>().ok().map(|integer| integer.to_string())
    }
}

fn integer_value(value: &Dynamic<'_>) -> Option<i64> {
    value
        .as_int()
        .and_then(|integer| integer.as_i64())
        .or_else(|| {
            value
                .as_bv()
                .filter(|bv| bv.get_size() == 64)
                .and_then(|bv| bv.as_u64())
                .map(|bits| bits as i64)
        })
}

pub fn decode_ieee754_f64(rendering: &str) -> Option<f64> {
    match rendering.trim() {
        "(_ +zero 11 53)" => return Some(0.0),
        "(_ -zero 11 53)" => return Some(-0.0),
        "(_ +oo 11 53)" => return Some(f64::INFINITY),
        "(_ -oo 11 53)" => return Some(f64::NEG_INFINITY),
        "(_ NaN 11 53)" => return Some(f64::NAN),
        _ => {}
    }
    let parts: Vec<_> = rendering
        .trim()
        .strip_prefix("(fp ")
        .and_then(|text| text.strip_suffix(')'))?
        .split_whitespace()
        .collect();
    if parts.len() != 3 {
        return None;
    }
    let sign_bits = parts[0].strip_prefix("#b")?;
    if sign_bits.len() != 1 {
        return None;
    }
    let sign = sign_bits.parse::<u64>().ok()?;
    let exponent_binary = parts[1].strip_prefix("#b")?;
    if exponent_binary.len() != 11 {
        return None;
    }
    let exponent_bits = u64::from_str_radix(exponent_binary, 2).ok()?;
    if exponent_bits >= (1 << 11) {
        return None;
    }
    let significand = if let Some(binary) = parts[2].strip_prefix("#b") {
        if binary.len() != 52 {
            return None;
        }
        u64::from_str_radix(binary, 2).ok()?
    } else {
        let hex = parts[2].strip_prefix("#x")?;
        if hex.len() != 13 {
            return None;
        }
        u64::from_str_radix(hex, 16).ok()?
    };
    let bits = (sign << 63) | (exponent_bits << 52) | significand;
    Some(f64::from_bits(bits))
}

fn helper_symbol_is_for_binding(
    symbol: &str,
    bindings: &HashSet<String>,
    module_env: &ModuleEnv,
) -> bool {
    bindings.iter().any(|binding| {
        let field = symbol_field(symbol, binding);
        !field.is_empty()
            && module_env
                .structs
                .values()
                .any(|structure| structure.fields.iter().any(|entry| entry.name == field))
    })
}

fn symbol_field(symbol: &str, binding: &str) -> String {
    symbol
        .strip_prefix(&format!("__struct_{binding}_"))
        .or_else(|| symbol.strip_prefix(&format!("{binding}_")))
        .unwrap_or("")
        .to_string()
}

fn is_internal_solver_symbol(name: &str) -> bool {
    name.starts_with("len_")
        || name.starts_with("__z3_arr_")
        || name.starts_with("__proj_")
        || name.starts_with("__mumei_struct_")
        || name.starts_with("__mumei_tuple_result_") && name.ends_with("_arity")
        || name.starts_with("__call_")
        || name.starts_with("__call_result")
        || name.starts_with("__fresh_")
        || name.starts_with("track_")
        || name.starts_with("__alive_")
        || name.starts_with("__borrowed_")
        || name.starts_with("__exclusive_")
        || name.starts_with("__")
}

fn unraisable(
    rendering: String,
    lowering: &'static str,
    reason: &str,
) -> (String, &'static str, RaisedStatus) {
    (
        rendering,
        lowering,
        RaisedStatus::Unraisable {
            reason: reason.to_string(),
        },
    )
}

pub fn raise_atom_counterexample<'ctx>(
    model: &Model<'ctx>,
    atom: &Atom,
    env: &HashMap<String, Dynamic<'ctx>>,
    module_env: &ModuleEnv,
    include_result: bool,
) -> RaisedCounterexample {
    let mut bindings = Vec::new();
    for param in &atom.params {
        if let Some(value) = env.get(&param.name) {
            bindings.push((param.name.clone(), param.type_name.clone(), value.clone()));
        }
    }
    if include_result {
        if let Some(result) = env.get("result") {
            bindings.push((
                "result".to_string(),
                atom.return_type.clone(),
                result.clone(),
            ));
        }
    }
    RaisedCounterexample::from_bindings(model, bindings, env, module_env)
}

#[cfg(test)]
mod tests {
    use super::{decode_ieee754_f64, raise_model_value, RaisedCounterexample, RaisedStatus};
    use crate::parser::{RefinedType, Span};
    use crate::verification::ModuleEnv;
    use std::collections::HashMap;
    use z3::ast::{Ast, Bool, Dynamic, Int, Real, BV};
    use z3::{Config, Context, SatResult, Solver};

    #[test]
    fn non_dyadic_real_keeps_solver_rendering_as_unraisable() {
        let config = Config::new();
        let context = Context::new(&config);
        let solver = Solver::new(&context);
        let value = Real::new_const(&context, "r");
        solver.assert(&value._eq(&Real::from_real(&context, 1, 3)));
        assert_eq!(solver.check(), SatResult::Sat);
        let model = solver.get_model().unwrap();
        let dynamic: Dynamic = model.eval(&value, true).unwrap().into();
        let (rendering, lowering, status) =
            raise_model_value(&model, &dynamic, Some("f64"), &ModuleEnv::new());
        assert_eq!(lowering, "real_f64");
        assert_eq!(rendering, "(/ 1.0 3.0)");
        assert_eq!(
            status,
            RaisedStatus::Unraisable {
                reason: "no exact f64 rendering".to_string()
            }
        );
    }

    #[test]
    fn raises_exact_dyadic_real_as_source_float() {
        let config = Config::new();
        let context = Context::new(&config);
        let solver = Solver::new(&context);
        let value = Real::new_const(&context, "r");
        solver.assert(&value._eq(&Real::from_real(&context, 1, 2)));
        assert_eq!(solver.check(), SatResult::Sat);
        let model = solver.get_model().unwrap();
        let dynamic: Dynamic = model.eval(&value, true).unwrap().into();
        let (rendering, _, status) =
            raise_model_value(&model, &dynamic, Some("f64"), &ModuleEnv::new());
        assert_eq!(rendering, "0.5");
        assert_eq!(status, RaisedStatus::Raised);
    }

    #[test]
    fn ieee_decoder_handles_specials_and_finite_values() {
        assert_eq!(decode_ieee754_f64("(_ +zero 11 53)"), Some(0.0));
        assert_eq!(
            decode_ieee754_f64("(_ -zero 11 53)").unwrap().to_bits(),
            (-0.0f64).to_bits()
        );
        assert_eq!(decode_ieee754_f64("(_ +oo 11 53)"), Some(f64::INFINITY));
        assert_eq!(decode_ieee754_f64("(_ -oo 11 53)"), Some(f64::NEG_INFINITY));
        assert!(decode_ieee754_f64("(_ NaN 11 53)").unwrap().is_nan());
        assert_eq!(
            decode_ieee754_f64("(fp #b0 #b01111111111 #x0000000000000)"),
            Some(1.0)
        );
        let binary_significand = [
            "0000000000000",
            "0000000000000",
            "0000000000000",
            "0000000000000",
        ]
        .concat();
        assert_eq!(
            decode_ieee754_f64(&format!("(fp #b0 #b01111111111 #b{binary_significand})")),
            Some(1.0)
        );
        assert_eq!(
            decode_ieee754_f64(&format!("(fp #b1 #b00000000000 #b{binary_significand})"))
                .unwrap()
                .to_bits(),
            (-0.0f64).to_bits()
        );
        assert_eq!(decode_ieee754_f64("(/ 1.0 3.0)"), None);
    }

    #[test]
    fn tuple_component_is_named_and_unraisable() {
        let config = Config::new();
        let context = Context::new(&config);
        let solver = Solver::new(&context);
        solver.assert(&Bool::from_bool(&context, true));
        assert_eq!(solver.check(), SatResult::Sat);
        let model = solver.get_model().unwrap();
        let result: Dynamic = Int::new_const(&context, "result").into();
        let tuple_value: Dynamic = Int::new_const(&context, "__mumei_tuple_result_result_0").into();
        let env = HashMap::from([
            ("result".to_string(), result.clone()),
            ("__mumei_tuple_result_result_0".to_string(), tuple_value),
        ]);
        let raised = RaisedCounterexample::from_bindings(
            &model,
            vec![("result".to_string(), None, result)],
            &env,
            &ModuleEnv::new(),
        );
        let component = raised
            .values
            .iter()
            .find(|value| value.source_name == "result._0")
            .expect("tuple component should be retained");
        assert!(matches!(&component.status, RaisedStatus::Unraisable { .. }));
        assert!(!raised.is_complete());
    }

    #[test]
    fn fresh_callee_result_is_omitted_from_source_values() {
        let config = Config::new();
        let context = Context::new(&config);
        let solver = Solver::new(&context);
        solver.assert(&Bool::from_bool(&context, true));
        assert_eq!(solver.check(), SatResult::Sat);
        let model = solver.get_model().unwrap();
        let input: Dynamic = Int::new_const(&context, "x").into();
        let callee_result: Dynamic = Int::new_const(&context, "__call_result_positive_0").into();
        let env = HashMap::from([
            ("x".to_string(), input.clone()),
            ("__call_result_positive_0".to_string(), callee_result),
        ]);
        let raised = RaisedCounterexample::from_bindings(
            &model,
            vec![("x".to_string(), Some("i64".to_string()), input)],
            &env,
            &ModuleEnv::new(),
        );
        assert_eq!(
            raised.to_counterexample_json(),
            serde_json::json!({"x": "0"})
        );
        assert_eq!(
            raised.omitted_solver_symbols,
            vec!["__call_result_positive_0"]
        );
    }

    #[test]
    fn bitvec_i64_is_raised_as_signed_decimal() {
        let config = Config::new();
        let context = Context::new(&config);
        let solver = Solver::new(&context);
        let value: Dynamic = BV::from_i64(&context, -1, 64).into();
        solver.assert(&value.as_bv().unwrap()._eq(&BV::from_i64(&context, -1, 64)));
        assert_eq!(solver.check(), SatResult::Sat);
        let model = solver.get_model().unwrap();
        let evaluated = model.eval(&value, true).unwrap();
        let (rendering, lowering, status) =
            raise_model_value(&model, &evaluated, Some("i64"), &ModuleEnv::new());
        assert_eq!(rendering, "-1");
        assert_eq!(lowering, "bitvec_i64");
        assert_eq!(status, RaisedStatus::Raised);
    }

    #[test]
    fn refinement_type_name_is_retained_while_raising_its_base_value() {
        let config = Config::new();
        let context = Context::new(&config);
        let solver = Solver::new(&context);
        let value = Int::from_i64(&context, 7);
        solver.assert(&value._eq(&Int::from_i64(&context, 7)));
        assert_eq!(solver.check(), SatResult::Sat);
        let model = solver.get_model().unwrap();
        let dynamic: Dynamic = model.eval(&value, true).unwrap().into();
        let mut module_env = ModuleEnv::new();
        module_env.register_type(&RefinedType {
            name: "Nat".to_string(),
            _base_type: "i64".to_string(),
            operand: "v".to_string(),
            predicate_raw: "v >= 0".to_string(),
            unit: None,
            span: Span::default(),
        });
        module_env.register_type(&RefinedType {
            name: "PositiveNat".to_string(),
            _base_type: "Nat".to_string(),
            operand: "v".to_string(),
            predicate_raw: "v > 0".to_string(),
            unit: None,
            span: Span::default(),
        });
        let raised =
            super::raise_named_model_value(&model, "x", &dynamic, Some("PositiveNat"), &module_env);
        assert_eq!(raised.rendering, "7");
        assert_eq!(raised.source_type.as_deref(), Some("PositiveNat"));
        assert_eq!(raised.status, RaisedStatus::Raised);
    }

    #[test]
    fn old_state_without_pre_state_provenance_is_unraisable() {
        let config = Config::new();
        let context = Context::new(&config);
        let solver = Solver::new(&context);
        solver.assert(&Bool::from_bool(&context, true));
        assert_eq!(solver.check(), SatResult::Sat);
        let model = solver.get_model().unwrap();
        let result: Dynamic = Int::new_const(&context, "result").into();
        let old_value: Dynamic = Int::new_const(&context, "__old_balance").into();
        let env = HashMap::from([
            ("result".to_string(), result.clone()),
            ("__old_balance".to_string(), old_value),
        ]);
        let raised = RaisedCounterexample::from_bindings(
            &model,
            vec![("result".to_string(), None, result)],
            &env,
            &ModuleEnv::new(),
        );
        let old = raised
            .values
            .iter()
            .find(|value| value.source_name == "old(balance)")
            .expect("old state should be retained");
        assert!(matches!(&old.status, RaisedStatus::Unraisable { .. }));
    }
}
