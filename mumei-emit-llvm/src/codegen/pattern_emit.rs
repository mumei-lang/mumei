use crate::codegen::lowering::{bitpreserve_cast, resolve_param_type};
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::types::BasicTypeEnum;
use inkwell::values::BasicValueEnum;
use inkwell::IntPredicate;
use mumei_core::parser::Pattern;
use mumei_core::verification::{ModuleEnv, MumeiError, MumeiResult};
use std::collections::HashMap;

/// `[Color]` / `Option<T>` → base type name (`Color`, `Option`).
fn base_type_name(ty: &str) -> &str {
    let base = ty
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(ty);
    base.split('<').next().unwrap_or(base).trim()
}

/// Resolve the enum that owns `variant_name` for a pattern: the match
/// target's declared type (`enum_hint`) wins; otherwise a sole owner or
/// owners agreeing on index/arity/field types; otherwise a hard error.
/// `enums` is a HashMap, so a name-only scan would pick per-process.
fn pattern_enum<'a>(
    module_env: &'a ModuleEnv,
    variant_name: &str,
    enum_hint: Option<&str>,
) -> MumeiResult<Option<&'a mumei_core::parser::EnumDef>> {
    let hint = enum_hint.map(base_type_name);
    module_env.resolve_variant_owner_by_hint(variant_name, hint)
}

/// `E::V` → `V`; bare `V` → `V`. `EnumVariant.name` stores the leaf only,
/// so qualified pattern names compare by the leaf segment.
fn variant_leaf(variant_name: &str) -> &str {
    variant_name.rsplit("::").next().unwrap_or(variant_name)
}

/// The declared type of a variant payload field — used as the enum hint
/// for nested `Variant` field patterns (`Self` resolves to the owner).
fn variant_field_hint(
    enum_def: &mumei_core::parser::EnumDef,
    variant_name: &str,
    field_idx: usize,
    module_env: &ModuleEnv,
) -> Option<String> {
    let variant_name = variant_leaf(variant_name);
    let field_ty = enum_def
        .variants
        .iter()
        .find(|v| v.name == variant_name)?
        .field_types
        .get(field_idx)?
        .name
        .as_str();
    let resolved = if field_ty == enum_def.name {
        enum_def.name.clone()
    } else {
        module_env.resolve_base_type(field_ty)
    };
    let base = base_type_name(&resolved);
    module_env.get_enum(base).map(|_| base.to_string())
}

fn variant_field_type<'a>(
    context: &'a Context,
    variant_name: &str,
    field_idx: usize,
    module_env: &ModuleEnv,
    enum_hint: Option<&str>,
) -> MumeiResult<Option<BasicTypeEnum<'a>>> {
    let Some(enum_def) = pattern_enum(module_env, variant_name, enum_hint)? else {
        return Ok(None);
    };
    let variant_name = variant_leaf(variant_name);
    let variant = enum_def.variants.iter().find(|v| v.name == variant_name);
    let field_type = variant.and_then(|v| v.field_types.get(field_idx));
    Ok(field_type.map(|ft| resolve_param_type(context, Some(ft.name.as_str()), module_env)))
}

fn extract_variant_field_value<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    target: BasicValueEnum<'a>,
    variant_name: &str,
    field_idx: usize,
    module_env: &ModuleEnv,
    enum_hint: Option<&str>,
) -> MumeiResult<BasicValueEnum<'a>> {
    let extracted = if target.is_struct_value() {
        llvm!(builder.build_extract_value(
            target.into_struct_value(),
            (field_idx + 1) as u32,
            &format!("variant_payload_{}", field_idx),
        ))
    } else {
        context.i64_type().const_int(0, false).into()
    };

    if let Some(slot_ty) =
        variant_field_type(context, variant_name, field_idx, module_env, enum_hint)?
    {
        if extracted.get_type() != slot_ty {
            return bitpreserve_cast(builder, extracted, slot_ty);
        }
    }

    Ok(extracted)
}

pub(crate) fn compile_pattern_test<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    pattern: &Pattern,
    target: BasicValueEnum<'a>,
    _variables: &HashMap<String, BasicValueEnum<'a>>,
    module_env: &ModuleEnv,
    enum_hint: Option<&str>,
) -> MumeiResult<inkwell::values::IntValue<'a>> {
    match pattern {
        Pattern::Wildcard | Pattern::Variable(_) => Ok(context.bool_type().const_int(1, false)),
        Pattern::Literal(n) => {
            let target_int = target.into_int_value();
            let lit = context.i64_type().const_int(*n as u64, true);
            let cmp =
                llvm!(builder.build_int_compare(IntPredicate::EQ, target_int, lit, "pat_lit_eq"));
            Ok(cmp)
        }
        Pattern::Variant {
            variant_name,
            fields,
        } => {
            // Resolve the owning enum deterministically: the match target's
            // declared type wins, then sole/agreeing owners; a true
            // collision is a compile error rather than a per-process pick.
            let resolved_enum = pattern_enum(module_env, variant_name, enum_hint)?;
            let variant_name = variant_leaf(variant_name);
            let tag_val = if let Some(enum_def) = resolved_enum {
                enum_def
                    .variants
                    .iter()
                    .position(|v| v.name == variant_name)
                    .unwrap_or(0) as u64
            } else {
                variant_name
                    .bytes()
                    .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64))
            };

            let target_tag = if target.is_struct_value() {
                llvm!(builder.build_extract_value(target.into_struct_value(), 0, "pat_tag"))
                    .into_int_value()
            } else {
                target.into_int_value()
            };

            let tag_const = context.i64_type().const_int(tag_val, false);
            let tag_match = llvm!(builder.build_int_compare(
                IntPredicate::EQ,
                target_tag,
                tag_const,
                "pat_tag_eq"
            ));

            let mut result = tag_match;
            for (field_idx, field_pat) in fields.iter().enumerate() {
                match field_pat {
                    Pattern::Wildcard | Pattern::Variable(_) => {}
                    _ => {
                        // Nested Variant field patterns resolve against the
                        // field's declared type (`Self` -> the owner enum).
                        let nested_hint = resolved_enum.and_then(|e| {
                            variant_field_hint(e, variant_name, field_idx, module_env)
                        });
                        let field_val = extract_variant_field_value(
                            context,
                            builder,
                            target,
                            variant_name,
                            field_idx,
                            module_env,
                            enum_hint,
                        )?;
                        let field_test = compile_pattern_test(
                            context,
                            builder,
                            field_pat,
                            field_val,
                            _variables,
                            module_env,
                            nested_hint.as_deref(),
                        )?;
                        result = llvm!(builder.build_and(result, field_test, "pat_nested_and"));
                    }
                }
            }
            Ok(result)
        }
    }
}

/// パターンから変数バインドを variables に登録する（再帰的）。
/// - Variable(name) → target の値を name にバインド
/// - Variant の fields 内の Variable → payload から extract_value で取得
pub(crate) fn bind_pattern_variables<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    pattern: &Pattern,
    target: BasicValueEnum<'a>,
    variables: &mut HashMap<String, BasicValueEnum<'a>>,
    module_env: &ModuleEnv,
    enum_hint: Option<&str>,
) -> MumeiResult<()> {
    match pattern {
        Pattern::Variable(name) => {
            variables.insert(name.clone(), target);
            Ok(())
        }
        Pattern::Variant {
            variant_name,
            fields,
        } => {
            let resolved_enum = pattern_enum(module_env, variant_name, enum_hint)?;
            let variant_name = variant_leaf(variant_name);
            for (field_idx, field_pat) in fields.iter().enumerate() {
                let nested_hint = resolved_enum
                    .and_then(|e| variant_field_hint(e, variant_name, field_idx, module_env));
                match field_pat {
                    Pattern::Variable(fname) => {
                        let field_val = extract_variant_field_value(
                            context,
                            builder,
                            target,
                            variant_name,
                            field_idx,
                            module_env,
                            enum_hint,
                        )?;
                        variables.insert(fname.clone(), field_val);
                    }
                    Pattern::Variant { .. } => {
                        let nested_val = extract_variant_field_value(
                            context,
                            builder,
                            target,
                            variant_name,
                            field_idx,
                            module_env,
                            enum_hint,
                        )?;
                        bind_pattern_variables(
                            context,
                            builder,
                            field_pat,
                            nested_val,
                            variables,
                            module_env,
                            nested_hint.as_deref(),
                        )?;
                    }
                    _ => {}
                }
            }
            Ok(())
        }
        Pattern::Wildcard | Pattern::Literal(_) => Ok(()),
    }
}

// FieldAccess now threads the receiver type when available; keep name-only fallback as a last resort.
/// フィールド名のみから全構造体定義を走査してインデックスを検索（ネスト構造体用）
pub(crate) fn find_field_index_by_name(field_name: &str, module_env: &ModuleEnv) -> Option<u32> {
    for sdef in module_env.structs.values() {
        if let Some(pos) = sdef.fields.iter().position(|f| f.name == field_name) {
            return Some(pos as u32);
        }
    }
    None
}

/// 構造体定義からフィールド名のインデックスを検索
pub(crate) fn find_field_index(
    type_or_var_name: &str,
    field_name: &str,
    module_env: &ModuleEnv,
) -> Option<u32> {
    // ModuleEnv に登録された構造体を探索
    // var_name が構造体型名と一致する場合、または型名を推定
    if let Some(sdef) = module_env.get_struct(type_or_var_name) {
        return sdef
            .fields
            .iter()
            .position(|f| f.name == field_name)
            .map(|i| i as u32);
    }
    // FieldAccess now threads the receiver type when available; keep name-only fallback as a last resort.
    // フォールバック: 全構造体定義を走査してフィールド名が一致するものを探す
    for sdef in module_env.structs.values() {
        if let Some(pos) = sdef.fields.iter().position(|f| f.name == field_name) {
            return Some(pos as u32);
        }
    }
    None
}
