use super::module_env::ModuleEnv;
use super::translator::struct_field_key;
use crate::lowering::{lower, LoweredType};
use crate::parser::{Atom, EnumDef};
use serde_json::{json, Map, Number, Value};
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

impl RaisedValue {
    pub fn to_loss_json(&self) -> Value {
        loss_json_from_raised_rendering(
            &self.rendering,
            self.lowering,
            matches!(self.status, RaisedStatus::Raised),
        )
    }
}

pub fn loss_json_from_raised_rendering(rendering: &str, lowering: &str, is_raised: bool) -> Value {
    if !is_raised {
        return Value::String(rendering.to_string());
    }
    match lowering {
        "int" | "bitvec_i64" | "bitvec_i32" | "bitvec_u64" | "bitvec_u32" => rendering
            .parse::<i64>()
            .map(Number::from)
            .or_else(|_| rendering.parse::<u64>().map(Number::from))
            .map(Value::Number)
            .unwrap_or_else(|_| Value::String(rendering.to_string())),
        "real_f64" | "ieee754_f64" | "real_f32" | "ieee754_f32" => rendering
            .parse::<f64>()
            .ok()
            .and_then(Number::from_f64)
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(rendering.to_string())),
        "bool" => match rendering {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => Value::String(rendering.to_string()),
        },
        _ => Value::String(rendering.to_string()),
    }
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

    pub fn to_loss_json(&self) -> Value {
        let mut values = Map::new();
        for value in &self.values {
            values.insert(value.source_name.clone(), value.to_loss_json());
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
            let inferred_type = if source_type.is_none() {
                infer_untyped_struct_type(name, env, module_env)
            } else {
                Ok(None)
            };
            let source_type = source_type.clone().or_else(|| {
                inferred_type
                    .as_ref()
                    .ok()
                    .and_then(|inferred| inferred.clone())
            });
            let (rendering, lowering, status) = match inferred_type {
                Err(reason) => (
                    evaluated.to_string(),
                    "struct",
                    RaisedStatus::Unraisable { reason },
                ),
                Ok(_) if source_type.is_none() && has_untyped_array_metadata(name, env) => (
                    evaluated.to_string(),
                    "array",
                    RaisedStatus::Unraisable {
                        reason: "array metadata is present but its element type is unavailable"
                            .to_string(),
                    },
                ),
                Ok(_) => raise_model_value_with_env(
                    model,
                    &evaluated,
                    source_type.as_deref(),
                    module_env,
                    env,
                    Some(name),
                ),
            };
            raised.values.push(RaisedValue {
                source_name: name.clone(),
                solver_name: None,
                rendering,
                lowering,
                source_type,
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

fn infer_untyped_struct_type(
    binding: &str,
    env: &HashMap<String, Dynamic<'_>>,
    module_env: &ModuleEnv,
) -> Result<Option<String>, String> {
    let mut candidates = module_env
        .structs
        .iter()
        .filter_map(|(type_name, structure)| {
            let total_fields = structure.fields.len();
            if total_fields == 0 {
                return None;
            }
            let present_fields = structure
                .fields
                .iter()
                .filter(|field| {
                    env.contains_key(&struct_field_key(binding, &field.name))
                        || env.contains_key(&format!("{binding}_{}", field.name))
                })
                .count();
            (present_fields > 0).then(|| (type_name.clone(), present_fields, total_fields))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.0.cmp(&right.0));

    if candidates.is_empty() {
        let helper_prefix = format!("__struct_{binding}_");
        if env.keys().any(|symbol| symbol.starts_with(&helper_prefix)) {
            return Err(
                "struct field symbols are present but no matching struct definition is available"
                    .to_string(),
            );
        }
        return Ok(None);
    }
    if candidates.len() != 1 {
        return Err(
            "struct field symbols are ambiguous across multiple struct definitions".to_string(),
        );
    }

    let (type_name, present_fields, total_fields) = candidates.pop().expect("one candidate");
    if present_fields != total_fields {
        return Err(format!(
            "struct field symbols for `{binding}` are incomplete for type `{type_name}`"
        ));
    }
    Ok(Some(type_name))
}

fn has_untyped_array_metadata(binding: &str, env: &HashMap<String, Dynamic<'_>>) -> bool {
    env.contains_key(&format!("len_{binding}")) || env.contains_key(&format!("__z3_arr_{binding}"))
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
        Some(
            integer_type @ (LoweredType::I64
            | LoweredType::I32
            | LoweredType::U64
            | LoweredType::U32),
        ) if value.as_bv().is_some() => raise_integer_bitvector(value, raw, integer_type),
        Some(
            integer_type @ (LoweredType::I64
            | LoweredType::I32
            | LoweredType::U64
            | LoweredType::U32),
        ) if value.as_int().is_some() => raise_integer_value(value, raw, Some(integer_type)),
        None if value.as_int().is_some() => raise_integer_value(value, raw, None),
        Some(LoweredType::F64) if value.as_float().is_some() => {
            let float = value.as_float().expect("checked above");
            let sort = float.get_sort();
            if (sort.float_exponent_size(), sort.float_significand_size()) != (Some(11), Some(53)) {
                return unraisable(
                    raw,
                    "ieee754_f64",
                    &format!(
                        "expected IEEE Float(11, 53), found {}",
                        solver_sort_description(value)
                    ),
                );
            }
            decode_ieee754_f64(&raw)
                .map(|number| (format!("{number:?}"), "ieee754_f64", RaisedStatus::Raised))
                .unwrap_or_else(|| {
                    unraisable(
                        raw.clone(),
                        "ieee754_f64",
                        "unsupported IEEE 754 model rendering",
                    )
                })
        }
        Some(LoweredType::F64) if value.as_real().is_some() => {
            let exact = value
                .as_real()
                .and_then(|real| real.as_real())
                .and_then(|(numerator, denominator)| exact_dyadic_f64(numerator, denominator));
            exact
                .map(|number| (format!("{number:?}"), "real_f64", RaisedStatus::Raised))
                .unwrap_or_else(|| unraisable(raw.clone(), "real_f64", "no exact f64 rendering"))
        }
        Some(LoweredType::F32) if value.as_float().is_some() => {
            let float = value.as_float().expect("checked above");
            let sort = float.get_sort();
            if (sort.float_exponent_size(), sort.float_significand_size()) != (Some(8), Some(24)) {
                return unraisable(
                    raw,
                    "ieee754_f32",
                    &format!(
                        "expected IEEE Float(8, 24), found {}",
                        solver_sort_description(value)
                    ),
                );
            }
            decode_ieee754_f32(&raw)
                .map(|number| (format!("{number:?}"), "ieee754_f32", RaisedStatus::Raised))
                .unwrap_or_else(|| {
                    unraisable(
                        raw.clone(),
                        "ieee754_f32",
                        "unsupported IEEE 754 binary32 model rendering",
                    )
                })
        }
        Some(LoweredType::F32) if value.as_real().is_some() => {
            let exact = value
                .as_real()
                .and_then(|real| real.as_real())
                .and_then(|(numerator, denominator)| exact_dyadic_f32(numerator, denominator));
            exact
                .map(|number| (format!("{number:?}"), "real_f32", RaisedStatus::Raised))
                .unwrap_or_else(|| unraisable(raw.clone(), "real_f32", "no exact f32 rendering"))
        }
        Some(LoweredType::Bool) => value
            .as_bool()
            .and_then(|boolean| boolean.as_bool())
            .map(|boolean| (boolean.to_string(), "bool", RaisedStatus::Raised))
            .unwrap_or_else(|| {
                unraisable(raw.clone(), "bool", "could not decode the boolean value")
            }),
        Some(LoweredType::Str) => (raw, "string", RaisedStatus::Raised),
        Some(LoweredType::F32) => unraisable(
            raw,
            "f32",
            &format!(
                "expected a Real or IEEE Float(8, 24) for f32, found {}",
                solver_sort_description(value)
            ),
        ),
        Some(LoweredType::F64) => unraisable(
            raw,
            "f64",
            &format!(
                "expected a Real or IEEE Float(11, 53) for f64, found {}",
                solver_sort_description(value)
            ),
        ),
        Some(
            integer_type @ (LoweredType::I64
            | LoweredType::I32
            | LoweredType::U64
            | LoweredType::U32),
        ) => unraisable(
            raw,
            "int",
            &format!(
                "expected a Z3 Int or matching-width bit-vector for {}, found {}",
                integer_type_name(&integer_type),
                solver_sort_description(value)
            ),
        ),
        _ => unraisable(
            raw,
            "unknown",
            "no source lowering is available for this model value",
        ),
    }
}

fn integer_type_name(lowered: &LoweredType) -> &'static str {
    match lowered {
        LoweredType::I64 => "i64",
        LoweredType::I32 => "i32",
        LoweredType::U64 => "u64",
        LoweredType::U32 => "u32",
        _ => "integer",
    }
}

fn integer_bitvector_lowering(lowered: &LoweredType) -> Option<(&'static str, u32, bool)> {
    match lowered {
        LoweredType::I64 => Some(("bitvec_i64", 64, true)),
        LoweredType::I32 => Some(("bitvec_i32", 32, true)),
        LoweredType::U64 => Some(("bitvec_u64", 64, false)),
        LoweredType::U32 => Some(("bitvec_u32", 32, false)),
        _ => None,
    }
}

fn raise_integer_bitvector(
    value: &Dynamic<'_>,
    raw: String,
    lowered: LoweredType,
) -> (String, &'static str, RaisedStatus) {
    let (lowering, width, signed) =
        integer_bitvector_lowering(&lowered).expect("integer lowering has bit-vector metadata");
    let bv = value.as_bv().expect("caller checked for bit-vector");
    if bv.get_size() != width {
        return unraisable(
            raw,
            lowering,
            &format!(
                "expected a {width}-bit bit-vector for {}, found a {}-bit bit-vector",
                integer_type_name(&lowered),
                bv.get_size()
            ),
        );
    }
    let Some(bits) = bv
        .as_u64()
        .or_else(|| bv.as_i64().map(|value| value as u64))
    else {
        return unraisable(raw, lowering, "could not decode the bit-vector value");
    };
    let rendering = match (width, signed) {
        (64, true) => (bits as i64).to_string(),
        (32, true) => (bits as u32 as i32).to_string(),
        (64, false) => bits.to_string(),
        (32, false) => (bits as u32).to_string(),
        _ => unreachable!("integer lowering has a supported bit width"),
    };
    (rendering, lowering, RaisedStatus::Raised)
}

fn raise_integer_value(
    value: &Dynamic<'_>,
    raw: String,
    lowered: Option<LoweredType>,
) -> (String, &'static str, RaisedStatus) {
    let integer = value.as_int().expect("caller checked for integer");
    let value = integer
        .as_i64()
        .map(i128::from)
        .or_else(|| integer.as_u64().map(i128::from))
        .or_else(|| parse_solver_integer(&raw));
    let rendering = value.and_then(|value| match lowered {
        Some(LoweredType::I64) => i64::try_from(value).ok().map(|value| value.to_string()),
        Some(LoweredType::I32) => i32::try_from(value).ok().map(|value| value.to_string()),
        Some(LoweredType::U64) => u64::try_from(value).ok().map(|value| value.to_string()),
        Some(LoweredType::U32) => u32::try_from(value).ok().map(|value| value.to_string()),
        Some(_) => None,
        None => Some(value.to_string()),
    });
    rendering
        .map(|rendering| (rendering, "int", RaisedStatus::Raised))
        .unwrap_or_else(|| {
            unraisable(
                raw,
                "int",
                "integer is outside the declared source type range",
            )
        })
}

fn solver_sort_description(value: &Dynamic<'_>) -> String {
    if let Some(float) = value.as_float() {
        let sort = float.get_sort();
        return match (sort.float_exponent_size(), sort.float_significand_size()) {
            (Some(exponent), Some(significand)) => {
                format!("IEEE Float({exponent}, {significand})")
            }
            _ => "IEEE floating-point sort".to_string(),
        };
    }
    if value.as_int().is_some() {
        "Int".to_string()
    } else if value.as_real().is_some() {
        "Real".to_string()
    } else if let Some(bitvector) = value.as_bv() {
        format!("{}-bit bit-vector", bitvector.get_size())
    } else if value.as_bool().is_some() {
        "Bool".to_string()
    } else if value.as_string().is_some() {
        "String".to_string()
    } else {
        "unknown".to_string()
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
        let alias_key = format!("{flat_binding}_{}", field.name);
        let alternate_alias_key = format!("{binding}_{}", field.name);
        let Some(field_value) = env
            .get(&key)
            .or_else(|| env.get(&alternate_key))
            .or_else(|| env.get(&alias_key))
            .or_else(|| env.get(&alternate_alias_key))
        else {
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

fn exact_dyadic_f32(numerator: i64, denominator: i64) -> Option<f32> {
    let value = exact_dyadic_f64(numerator, denominator)?;
    let rounded = value as f32;
    (rounded.is_finite() && rounded as f64 == value).then_some(rounded)
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

fn parse_solver_integer(raw: &str) -> Option<i128> {
    if let Some(inner) = raw
        .strip_prefix("(- ")
        .and_then(|value| value.strip_suffix(')'))
    {
        inner.parse::<i128>().ok()?.checked_neg()
    } else {
        raw.parse::<i128>().ok()
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

fn decode_ieee754_f32(rendering: &str) -> Option<f32> {
    match rendering.trim() {
        "(_ +zero 8 24)" => return Some(0.0),
        "(_ -zero 8 24)" => return Some(-0.0),
        "(_ +oo 8 24)" => return Some(f32::INFINITY),
        "(_ -oo 8 24)" => return Some(f32::NEG_INFINITY),
        "(_ NaN 8 24)" => return Some(f32::NAN),
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
    let sign = sign_bits.parse::<u32>().ok()?;
    let exponent_bits = if let Some(binary) = parts[1].strip_prefix("#b") {
        if binary.len() != 8 {
            return None;
        }
        u32::from_str_radix(binary, 2).ok()?
    } else {
        let hex = parts[1].strip_prefix("#x")?;
        if hex.len() != 2 {
            return None;
        }
        u32::from_str_radix(hex, 16).ok()?
    };
    let significand = if let Some(binary) = parts[2].strip_prefix("#b") {
        if binary.len() != 23 {
            return None;
        }
        u32::from_str_radix(binary, 2).ok()?
    } else {
        let hex = parts[2].strip_prefix("#x")?;
        if hex.len() != 6 {
            return None;
        }
        u32::from_str_radix(hex, 16).ok()?
    };
    if significand >= (1 << 23) {
        return None;
    }
    let bits = (sign << 31) | (exponent_bits << 23) | significand;
    Some(f32::from_bits(bits))
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
    use super::{
        decode_ieee754_f64, raise_model_value, RaisedCounterexample, RaisedStatus, RaisedValue,
    };
    use crate::parser::{RefinedType, Span};
    use crate::verification::ModuleEnv;
    use serde_json::Value;
    use std::collections::HashMap;
    use z3::ast::{Ast, Bool, Dynamic, Float, Int, Real, BV};
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
    fn raises_non_i64_integer_sorts_as_decimal() {
        let config = Config::new();
        let context = Context::new(&config);
        let solver = Solver::new(&context);
        solver.assert(&Bool::from_bool(&context, true));
        assert_eq!(solver.check(), SatResult::Sat);
        let model = solver.get_model().unwrap();

        for (type_name, value, expected) in [
            (
                "u64",
                Int::from_u64(&context, u64::MAX),
                u64::MAX.to_string(),
            ),
            ("i32", Int::from_i64(&context, -17), "-17".to_string()),
            (
                "u32",
                Int::from_u64(&context, u64::from(u32::MAX)),
                u32::MAX.to_string(),
            ),
        ] {
            let dynamic: Dynamic = value.into();
            let evaluated = model.eval(&dynamic, true).unwrap();
            let (rendering, lowering, status) =
                raise_model_value(&model, &evaluated, Some(type_name), &ModuleEnv::new());
            assert_eq!(rendering, expected, "{type_name}");
            assert_eq!(lowering, "int", "{type_name}");
            assert_eq!(status, RaisedStatus::Raised, "{type_name}");
        }
    }

    #[test]
    fn loss_json_preserves_unsigned_numbers_and_fail_closed_strings() {
        let unsigned = RaisedValue {
            source_name: "x".to_string(),
            solver_name: None,
            rendering: u64::MAX.to_string(),
            lowering: "bitvec_u64",
            source_type: Some("u64".to_string()),
            status: RaisedStatus::Raised,
        };
        assert_eq!(unsigned.to_loss_json(), Value::Number(u64::MAX.into()));

        let unraisable = RaisedValue {
            source_name: "x".to_string(),
            solver_name: None,
            rendering: "solver-value".to_string(),
            lowering: "int",
            source_type: Some("i64".to_string()),
            status: RaisedStatus::Unraisable {
                reason: "unsupported integer range".to_string(),
            },
        };
        assert_eq!(
            unraisable.to_loss_json(),
            Value::String("solver-value".to_string())
        );

        let structured = RaisedValue {
            source_name: "shape".to_string(),
            solver_name: None,
            rendering: "Shape::Circle(1)".to_string(),
            lowering: "enum",
            source_type: Some("Shape".to_string()),
            status: RaisedStatus::Raised,
        };
        assert_eq!(
            structured.to_loss_json(),
            Value::String("Shape::Circle(1)".to_string())
        );
    }

    #[test]
    fn bitvector_integer_types_use_declared_signedness_and_width() {
        let config = Config::new();
        let context = Context::new(&config);
        let solver = Solver::new(&context);
        solver.assert(&Bool::from_bool(&context, true));
        assert_eq!(solver.check(), SatResult::Sat);
        let model = solver.get_model().unwrap();

        for (type_name, value, expected, expected_lowering) in [
            (
                "u64",
                BV::from_u64(&context, u64::MAX, 64),
                u64::MAX.to_string(),
                "bitvec_u64",
            ),
            (
                "i32",
                BV::from_i64(&context, -1, 32),
                "-1".to_string(),
                "bitvec_i32",
            ),
            (
                "u32",
                BV::from_u64(&context, u64::from(u32::MAX), 32),
                u32::MAX.to_string(),
                "bitvec_u32",
            ),
        ] {
            let dynamic: Dynamic = value.into();
            let evaluated = model.eval(&dynamic, true).unwrap();
            let (rendering, lowering, status) =
                raise_model_value(&model, &evaluated, Some(type_name), &ModuleEnv::new());
            assert_eq!(rendering, expected, "{type_name}");
            assert_eq!(lowering, expected_lowering, "{type_name}");
            assert_eq!(status, RaisedStatus::Raised, "{type_name}");
        }
    }

    #[test]
    fn f32_values_follow_only_matching_real_or_ieee_sorts() {
        let config = Config::new();
        let context = Context::new(&config);
        let solver = Solver::new(&context);
        let real = Real::from_real(&context, 1, 2);
        let ieee = Float::from_f32(&context, 0.5);
        let integer = Int::from_i64(&context, 1);
        solver.assert(&Bool::from_bool(&context, true));
        assert_eq!(solver.check(), SatResult::Sat);
        let model = solver.get_model().unwrap();

        let real: Dynamic = model.eval(&real, true).unwrap().into();
        let (rendering, lowering, status) =
            raise_model_value(&model, &real, Some("f32"), &ModuleEnv::new());
        assert_eq!(rendering, "0.5");
        assert_eq!(lowering, "real_f32");
        assert_eq!(status, RaisedStatus::Raised);

        let ieee: Dynamic = model.eval(&ieee, true).unwrap().into();
        let (rendering, lowering, status) =
            raise_model_value(&model, &ieee, Some("f32"), &ModuleEnv::new());
        assert_eq!(rendering, "0.5");
        assert_eq!(lowering, "ieee754_f32");
        assert_eq!(status, RaisedStatus::Raised);

        let integer: Dynamic = model.eval(&integer, true).unwrap().into();
        let (rendering, lowering, status) =
            raise_model_value(&model, &integer, Some("f32"), &ModuleEnv::new());
        assert_eq!(rendering, "1");
        assert_eq!(lowering, "f32");
        assert_eq!(
            status,
            RaisedStatus::Unraisable {
                reason: "expected a Real or IEEE Float(8, 24) for f32, found Int".to_string()
            }
        );
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
