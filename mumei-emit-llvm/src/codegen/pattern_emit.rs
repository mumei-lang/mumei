use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::values::BasicValueEnum;
use inkwell::IntPredicate;
use mumei_core::parser::Pattern;
use mumei_core::verification::{ModuleEnv, MumeiError, MumeiResult};
use std::collections::HashMap;

pub(crate) fn compile_pattern_test<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    pattern: &Pattern,
    target: BasicValueEnum<'a>,
    _variables: &HashMap<String, BasicValueEnum<'a>>,
    module_env: &ModuleEnv,
) -> MumeiResult<inkwell::values::IntValue<'a>> {
    match pattern {
        Pattern::Wildcard | Pattern::Variable(_) => {
            // 常にマッチ
            Ok(context.bool_type().const_int(1, false))
        }
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
            // Plan 14: Enum variant pattern matching with payload support
            let tag_val = if let Some(enum_def) = module_env.find_enum_by_variant(variant_name) {
                enum_def
                    .variants
                    .iter()
                    .position(|v| v.name == *variant_name)
                    .unwrap_or(0) as u64
            } else {
                // Enum 定義が見つからない場合はハッシュベースのフォールバック
                variant_name
                    .bytes()
                    .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64))
            };

            // Extract tag from target (struct or int)
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

            // ネストパターンの再帰処理: 各フィールドの条件を AND 結合
            let mut result = tag_match;
            for (field_idx, field_pat) in fields.iter().enumerate() {
                match field_pat {
                    Pattern::Wildcard | Pattern::Variable(_) => {
                        // 常にマッチ → AND しても変わらない
                    }
                    _ => {
                        // Plan 14: Extract actual payload field from struct
                        let field_val = if target.is_struct_value() {
                            llvm!(builder.build_extract_value(
                                target.into_struct_value(),
                                (field_idx + 1) as u32,
                                &format!("pat_payload_{}", field_idx)
                            ))
                        } else {
                            context.i64_type().const_int(0, false).into()
                        };
                        let field_test = compile_pattern_test(
                            context, builder, field_pat, field_val, _variables, module_env,
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
/// - Variant の fields 内の Variable → Plan 14: payload から extract_value で取得
pub(crate) fn bind_pattern_variables<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    pattern: &Pattern,
    target: BasicValueEnum<'a>,
    variables: &mut HashMap<String, BasicValueEnum<'a>>,
) {
    match pattern {
        Pattern::Variable(name) => {
            variables.insert(name.clone(), target);
        }
        Pattern::Variant {
            variant_name: _,
            fields,
        } => {
            for (field_idx, field_pat) in fields.iter().enumerate() {
                match field_pat {
                    Pattern::Variable(fname) => {
                        // Plan 14: Extract payload field from tagged union struct
                        if target.is_struct_value() {
                            if let Ok(field_val) = builder.build_extract_value(
                                target.into_struct_value(),
                                (field_idx + 1) as u32,
                                &format!("bind_{}", fname),
                            ) {
                                variables.insert(fname.clone(), field_val);
                            }
                        } else {
                            // Fallback for non-struct targets (legacy tag-only enums)
                            let dummy: BasicValueEnum =
                                context.i64_type().const_int(0, false).into();
                            variables.insert(fname.clone(), dummy);
                        }
                    }
                    Pattern::Variant { .. } => {
                        // ネストした Variant: 再帰的にバインド
                        let nested_val = if target.is_struct_value() {
                            builder
                                .build_extract_value(
                                    target.into_struct_value(),
                                    (field_idx + 1) as u32,
                                    &format!("nested_payload_{}", field_idx),
                                )
                                .unwrap_or(context.i64_type().const_int(0, false).into())
                        } else {
                            context.i64_type().const_int(0, false).into()
                        };
                        bind_pattern_variables(context, builder, field_pat, nested_val, variables);
                    }
                    _ => {}
                }
            }
        }
        Pattern::Wildcard | Pattern::Literal(_) => {
            // バインドなし
        }
    }
}

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
    // フォールバック: 全構造体定義を走査してフィールド名が一致するものを探す
    for sdef in module_env.structs.values() {
        if let Some(pos) = sdef.fields.iter().position(|f| f.name == field_name) {
            return Some(pos as u32);
        }
    }
    None
}
