#![allow(unused_imports)]
use super::super::support::*;
use super::super::*;
use super::*;
use crate::lowering::{lower, LoweredType};
use crate::parser::EnumVariant;
use serde_json::json;

/// Rebuild `sdef`'s direct field list from a flat per-leaf value list —
/// the list a composite enum payload flattens into, in `field_leaf_types`
/// order. A struct-typed field becomes a fresh inner handle whose own
/// fields are bound recursively, so `handle.inner.x` projects a declared
/// field (`inner`) instead of only seeing flat `inner_x` leaves — the same
/// shape a param-seeded nested struct has.
pub(crate) fn rebuild_payload_direct_fields<'a>(
    ctx: &'a Context,
    vc: &VCtx<'a>,
    env: &mut Env<'a>,
    sdef: &StructDef,
    leaves: &[Dynamic<'a>],
) -> Option<Vec<(String, Dynamic<'a>)>> {
    let mut direct = Vec::with_capacity(sdef.fields.len());
    let mut cursor = 0usize;
    for field in &sdef.fields {
        let field_leaves = datatype::field_leaf_types(&field.type_name, vc.module_env)?;
        let width = field_leaves.len();
        let slice = leaves.get(cursor..cursor + width)?;
        cursor += width;
        if datatype::array_payload_element_type(&field_leaves).is_some() {
            let array = slice.first()?.as_array()?;
            let len = slice.get(1)?.clone();
            super::z3_types::register_array_value_len(env, &array, len);
            direct.push((field.name.clone(), slice[0].clone()));
            continue;
        }
        // Aliases resolve like the variant-field lookup above does — an
        // un-resolved name misses `get_struct` and falls into the scalar
        // path, where an empty struct's zero-width slice would panic on
        // `slice[0]` (and a non-empty one would bind a leaf, not a handle).
        let resolved_ty = vc.module_env.resolve_base_type(&field.type_name);
        if let Some(inner) = vc.module_env.structs.get(&resolved_ty) {
            let inner_handle =
                crate::verification::translator::fresh_struct_handle(ctx, &inner.name);
            let inner_name = inner_handle.decl().name();
            let inner_direct = rebuild_payload_direct_fields(ctx, vc, env, inner, slice)?;
            crate::verification::translator::bind_struct_fields(env, &inner_name, &inner_direct);
            direct.push((field.name.clone(), inner_handle));
        } else {
            direct.push((field.name.clone(), slice.first()?.clone()));
        }
    }
    (cursor == leaves.len()).then_some(direct)
}

pub(crate) fn pattern_to_z3_condition<'a>(
    ctx: &'a Context,
    pattern: &Pattern,
    target: &Dynamic<'a>,
    env: &mut Env<'a>,
    vc: &VCtx<'a>,
    solver_opt: Option<&Solver<'a>>,
    decl_hint: Option<&str>,
) -> MumeiResult<Bool<'a>> {
    match pattern {
        Pattern::Wildcard | Pattern::Variable(_) => Ok(Bool::from_bool(ctx, true)),
        Pattern::Literal(n) => {
            // Compare in the target's own sort: under `--bitvec-i64` (or any
            // contract that forces BV encoding, e.g. bitwise ops) `as_int()`
            // fails and an unconstrained `__match_target` would silently make
            // every literal arm unreachable.
            if let Some(target_bv) = target.as_bv() {
                let lit = BV::from_i64(ctx, *n, target_bv.get_size());
                return Ok(target_bv._eq(&lit));
            }
            let target_int = target
                .as_int()
                .unwrap_or(Int::new_const(ctx, "__match_target"));
            let lit = Int::from_i64(ctx, *n);
            Ok(target_int._eq(&lit))
        }
        Pattern::Variant {
            variant_name,
            fields,
        } => {
            // Qualified `E::V` names resolve by the leaf after the owner
            // check inside `resolve_variant_owner`.
            let variant_leaf = datatype::variant_leaf(variant_name);
            // P10-C: a `Datatype`-sorted target matches by the variant's
            // `is-<V>` tester, and payload fields project through the real
            // selectors — so a `Str` field binds as a Z3 `String` and a field
            // pattern like `"x"` can actually constrain it.
            if target.as_datatype().is_some() {
                if let Some((sort, is_v, variant_idx, enum_def)) =
                    datatype::variant_tester_condition(vc, target, variant_name)
                {
                    let mut field_conditions: Vec<Bool> = vec![is_v.clone()];
                    // arity comes from the matched sort's accessors — the sort
                    // already disambiguates same-named variants across enums.
                    let enum_def = Some(enum_def);
                    for (i, field_pattern) in fields.iter().enumerate() {
                        // A struct-typed payload flattens into `width`
                        // consecutive accessor leaves; the bound symbol is a
                        // fresh struct handle over the selector applies.
                        let (off, width) = enum_def
                            .and_then(|edef| {
                                datatype::payload_leaf_range(edef, vc.module_env, variant_idx, i)
                            })
                            .unwrap_or((i, 1));
                        let payload_field_type = enum_def.and_then(|edef| {
                            edef.variants[variant_idx]
                                .field_types
                                .get(i)
                                .map(datatype::payload_field_type_name)
                        });
                        let payload_leaves = payload_field_type.as_ref().and_then(|field_type| {
                            datatype::field_leaf_types(field_type, vc.module_env)
                        });
                        let has_array_payload =
                            payload_leaves.as_ref().is_some_and(|leaves| {
                                datatype::array_payload_element_type(leaves).is_some()
                            }) || payload_field_type.as_ref().is_some_and(|field_type| {
                                datatype::struct_array_payload_element_type(
                                    field_type,
                                    vc.module_env,
                                )
                                .is_some()
                            });
                        if has_array_payload {
                            if !matches!(field_pattern, Pattern::Variable(_) | Pattern::Wildcard) {
                                return Err(MumeiError::verification(format!(
                                    "match pattern on array payload field '{variant_leaf}.{i}' must be a variable or '_'"
                                )));
                            }
                            let length_offset = payload_leaves
                                .as_ref()
                                .and_then(|leaves| {
                                    datatype::array_payload_leaf_pairs(leaves).find_map(
                                        |(array_index, _)| {
                                            datatype::array_payload_length_index(
                                                leaves,
                                                array_index,
                                            )
                                        },
                                    )
                                })
                                .unwrap_or(1);
                            let Some(len) = datatype::variant_selector_apply(
                                &sort,
                                variant_idx,
                                off + length_offset,
                                target,
                            ) else {
                                return Err(MumeiError::verification(
                                    "array payload length selector is unavailable",
                                ));
                            };
                            if let (Some(solver), Some(nonnegative)) =
                                (solver_opt, nonneg_constraint(ctx, &len))
                            {
                                solver.assert(&is_v.implies(&nonnegative));
                            }
                            field_conditions.push(Bool::from_bool(ctx, true));
                            continue;
                        }
                        let sdef = enum_def.and_then(|edef| {
                            // The pattern may bind more fields than the
                            // variant declares — `V(a, b)` on `V(i64)` —
                            // so index safely; an out-of-range slot falls
                            // through to the scalar fallback const. An
                            // enum-typed field is a single inner-datatype
                            // accessor, not a flattened struct — it must
                            // stay on the scalar path so a nested
                            // `Pair::P(..)` pattern recurses on the inner
                            // enum (and so a prelude struct of the same
                            // name cannot hijack the slot).
                            let ft = datatype::payload_field_type_name(
                                edef.variants[variant_idx].field_types.get(i)?,
                            );
                            let resolved = vc.module_env.resolve_base_type(&ft);
                            if vc.module_env.enums.contains_key(&resolved) {
                                return None;
                            }
                            vc.module_env.structs.get(&resolved)
                        });
                        if let Some(sdef) = sdef {
                            // Nested payloads flatten deeper than the direct
                            // fields — leaf names come from the same source
                            // the accessors were declared with
                            // (`inner_x` for `b.inner.x`).
                            let leaf_types = enum_def.and_then(|edef| {
                                datatype::field_leaf_types(
                                    &datatype::payload_field_type_name(
                                        edef.variants[variant_idx].field_types.get(i)?,
                                    ),
                                    vc.module_env,
                                )
                            });
                            let mut leaves: Vec<(String, Dynamic)> = Vec::with_capacity(width);
                            for j in off..off + width {
                                let leaf_name = leaf_types
                                    .as_ref()
                                    .and_then(|lt| lt.get(j - off))
                                    .map(|(n, _)| n.clone())
                                    .unwrap_or_else(|| sdef.fields[j - off].name.clone());
                                let leaf =
                                    datatype::variant_selector_apply(&sort, variant_idx, j, target)
                                        .unwrap_or_else(|| {
                                            Int::new_const(
                                                ctx,
                                                format!("__proj_{}_{}_{}", variant_leaf, i, j)
                                                    .as_str(),
                                            )
                                            .into()
                                        });
                                leaves.push((leaf_name, leaf));
                            }
                            if let (Some(solver), Some(leaf_types)) =
                                (solver_opt, leaf_types.as_ref())
                            {
                                let leaf_values: Vec<Dynamic> =
                                    leaves.iter().map(|(_, value)| value.clone()).collect();
                                datatype::assert_payload_array_lengths(
                                    vc,
                                    solver,
                                    &is_v,
                                    leaf_types,
                                    &leaf_values,
                                );
                                let recursive_payload = enum_def.is_some_and(|edef| {
                                    datatype::is_self_recursive_adt(edef, vc.module_env)
                                        && edef.variants[variant_idx]
                                            .field_types
                                            .get(i)
                                            .is_some_and(|field_type| {
                                                datatype::recursive_field_target(
                                                    edef,
                                                    field_type,
                                                    vc.module_env,
                                                )
                                                .is_none()
                                            })
                                });
                                if recursive_payload {
                                    for ((_, base), (_, value)) in leaf_types.iter().zip(&leaves) {
                                        let domain = if base.starts_with("enum:") {
                                            datatype::enum_value_leaf_domain(vc, value)
                                        } else {
                                            datatype::scalar_int_leaf_domain(vc, base, value)
                                        };
                                        if let Some(domain) = domain {
                                            solver.assert(&is_v.implies(&domain));
                                        }
                                    }
                                }
                            }
                            let handle = crate::verification::translator::fresh_struct_handle(
                                ctx, &sdef.name,
                            );
                            let handle_name = handle.decl().name();
                            // Bind the declared direct fields — a
                            // struct-typed field projects to a fresh inner
                            // handle so `h.inner.x` resolves; the flat leaf
                            // keys stay off the handle itself so
                            // `project_struct_fields` sees the declared
                            // fields, not `inner_x`-style leaves.
                            let leaf_vals: Vec<Dynamic> =
                                leaves.iter().map(|(_, v)| v.clone()).collect();
                            let Some(direct) =
                                rebuild_payload_direct_fields(ctx, vc, env, sdef, &leaf_vals)
                            else {
                                return Err(MumeiError::verification(
                                    "could not rebuild struct enum payload fields",
                                ));
                            };
                            crate::verification::translator::bind_struct_fields(
                                env,
                                &handle_name,
                                &direct,
                            );
                            env.insert(format!("__proj_{}_{}", variant_leaf, i), handle.clone());
                            match field_pattern {
                                Pattern::Variable(_) | Pattern::Wildcard => {
                                    field_conditions.push(Bool::from_bool(ctx, true));
                                }
                                _ => {
                                    return Err(MumeiError::verification(format!(
                                        "match pattern on composite payload field '{variant_name}.{i}' must be a variable or '_'"
                                    )));
                                }
                            }
                            continue;
                        }
                        let field_sym: Dynamic =
                            datatype::variant_selector_apply(&sort, variant_idx, off, target)
                                .unwrap_or_else(|| {
                                    Int::new_const(
                                        ctx,
                                        format!("__proj_{}_{}", variant_leaf, i).as_str(),
                                    )
                                    .into()
                                });
                        if let (Some(solver), Some(edef), Some(field_type)) = (
                            solver_opt,
                            enum_def,
                            enum_def.and_then(|edef| edef.variants[variant_idx].field_types.get(i)),
                        ) {
                            if datatype::is_self_recursive_adt(edef, vc.module_env)
                                && datatype::recursive_field_target(edef, field_type, vc.module_env)
                                    .is_none()
                            {
                                let type_name = datatype::payload_field_type_name(field_type);
                                if let Some(leaf_types) =
                                    datatype::field_leaf_types(&type_name, vc.module_env)
                                {
                                    for (offset, (_, base)) in leaf_types.iter().enumerate() {
                                        let leaf = if offset == 0 {
                                            Some(field_sym.clone())
                                        } else {
                                            datatype::variant_selector_apply(
                                                &sort,
                                                variant_idx,
                                                off + offset,
                                                target,
                                            )
                                        };
                                        let Some(leaf) = leaf else {
                                            continue;
                                        };
                                        let domain = if base.starts_with("enum:") {
                                            datatype::enum_value_leaf_domain(vc, &leaf)
                                        } else {
                                            datatype::scalar_int_leaf_domain(vc, base, &leaf)
                                        };
                                        if let Some(domain) = domain {
                                            solver.assert(&is_v.implies(&domain));
                                        }
                                    }
                                }
                            }
                        }
                        env.insert(format!("__proj_{}_{}", variant_leaf, i), field_sym.clone());
                        let field_cond = pattern_to_z3_condition(
                            ctx,
                            field_pattern,
                            &field_sym,
                            env,
                            vc,
                            solver_opt,
                            None,
                        )?;
                        field_conditions.push(field_cond);
                    }
                    let cond_refs: Vec<&Bool> = field_conditions.iter().collect();
                    return Ok(Bool::and(ctx, &cond_refs));
                }
            }
            if let Some(enum_def) =
                datatype::resolve_variant_owner(vc, target, variant_name, decl_hint)?
            {
                let variant_idx = enum_def
                    .variants
                    .iter()
                    .position(|v| v.name == variant_leaf)
                    .unwrap_or(0) as i64;

                let tag = target
                    .as_int()
                    .unwrap_or(Int::new_const(ctx, "__match_tag"));
                let tag_match = tag._eq(&Int::from_i64(ctx, variant_idx));

                let variant_def = &enum_def.variants[variant_idx as usize];
                let mut field_conditions: Vec<Bool> = vec![tag_match];

                for (i, field_pattern) in fields.iter().enumerate() {
                    // Projector シンボル: __proj_{Enum}_{Variant}_{i}
                    // enum 名を含めることで variant 名の enum 間衝突
                    // （prelude List vs user IntList の Cons 等）でも一意になり、
                    // `match t` した束縛 tail も名前から宣言型を復元できる。
                    let proj_name = format!("__proj_{}_{}_{}", enum_def.name, variant_leaf, i);
                    let field_sym: Dynamic = if i < variant_def.fields.len() {
                        let field_type = &variant_def.fields[i];
                        // 再帰的 ADT: フィールド型が自身の Enum なら tag として Int を使用
                        let base = if *field_type == enum_def.name {
                            "i64".to_string() // 再帰フィールドは tag 値
                        } else {
                            vc.module_env.resolve_base_type(field_type)
                        };
                        match base.as_str() {
                            "f64" => Float::new_const(ctx, proj_name.as_str(), 11, 53).into(),
                            _ => Int::new_const(ctx, proj_name.as_str()).into(),
                        }
                    } else {
                        Int::new_const(ctx, proj_name.as_str()).into()
                    };

                    // env にも projector を登録（body 内で参照可能にする）
                    env.insert(proj_name.clone(), field_sym.clone());

                    // フィールドの宣言型を次段の hint として渡す
                    // （再帰 `Self` フィールド → 自身の enum 名）
                    let field_hint: Option<String> = if i < variant_def.fields.len() {
                        let ft = &variant_def.fields[i];
                        let resolved = if *ft == enum_def.name {
                            enum_def.name.clone()
                        } else {
                            vc.module_env.resolve_base_type(ft)
                        };
                        vc.module_env.get_enum(&resolved).map(|_| resolved)
                    } else {
                        None
                    };

                    // enum-typed fields are constrained to their tag domain —
                    // a `Self` tail stays in `0..enum_def.variants.len()`, and
                    // a field of another enum (`Outer::Wrap(m)`) stays in
                    // `0..Mine.len()` so a nested match sees real coverage.
                    if let Some(fenum_name) = &field_hint {
                        if let Some(fenum) = vc.module_env.get_enum(fenum_name) {
                            if let Some(solver) = solver_opt {
                                if let Some(field_int) = field_sym.as_int() {
                                    let n = fenum.variants.len() as i64;
                                    solver.assert(&field_int.ge(&Int::from_i64(ctx, 0)));
                                    solver.assert(&field_int.lt(&Int::from_i64(ctx, n)));
                                }
                            }
                        }
                    }
                    // 再帰的にフィールドパターンの条件を生成
                    let field_cond = pattern_to_z3_condition(
                        ctx,
                        field_pattern,
                        &field_sym,
                        env,
                        vc,
                        solver_opt,
                        field_hint.as_deref(),
                    )?;
                    field_conditions.push(field_cond);
                }

                let cond_refs: Vec<&Bool> = field_conditions.iter().collect();
                Ok(Bool::and(ctx, &cond_refs))
            } else {
                let tag = target
                    .as_int()
                    .unwrap_or(Int::new_const(ctx, "__match_tag"));
                let hash = variant_leaf
                    .bytes()
                    .fold(0i64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as i64));
                Ok(tag._eq(&Int::from_i64(ctx, hash)))
            }
        }
    }
}

/// パターンから変数バインドを env に登録する（再帰的: ネストパターン対応）
///
/// Phase 1-B: projector シンボルを使ったバインド
/// - Variable(name) → target の値を name にバインド
/// - Variant の fields 内の Variable → projector シンボル `__proj_{Variant}_{i}` にバインド
/// - Variant の fields 内の Variant → 再帰的に projector を生成してバインド
pub(crate) fn pattern_bind_variables<'a>(
    pattern: &Pattern,
    target: &Dynamic<'a>,
    env: &mut Env<'a>,
    vc: &VCtx<'a>,
    solver_opt: Option<&Solver<'a>>,
    decl_hint: Option<&str>,
    bound: &mut std::collections::HashSet<String>,
) {
    let module_env = vc.module_env;
    match pattern {
        Pattern::Variable(name) => {
            env.insert(name.clone(), target.clone());
        }
        Pattern::Variant {
            variant_name,
            fields,
        } => {
            let variant_leaf = datatype::variant_leaf(variant_name);
            // P10-C: datatype targets bind fields through the variant's
            // selectors so the bound symbol carries the payload's real sort
            // (String/Real/Bool), not an unconstrained Int projector.
            if target.as_datatype().is_some() {
                if let Some((sort, is_v, variant_idx, enum_def)) =
                    datatype::variant_tester_condition(vc, target, variant_name)
                {
                    {
                        let enum_def = Some(enum_def);
                        for (i, field_pattern) in fields.iter().enumerate() {
                            let (off, width) = enum_def
                                .and_then(|edef| {
                                    datatype::payload_leaf_range(
                                        edef,
                                        vc.module_env,
                                        variant_idx,
                                        i,
                                    )
                                })
                                .unwrap_or((i, 1));
                            let payload_field_type = enum_def.and_then(|edef| {
                                edef.variants[variant_idx]
                                    .field_types
                                    .get(i)
                                    .map(datatype::payload_field_type_name)
                            });
                            let payload_leaves =
                                payload_field_type.as_ref().and_then(|field_type| {
                                    datatype::field_leaf_types(field_type, vc.module_env)
                                });
                            let array_element = payload_leaves
                                .as_ref()
                                .and_then(|leaves| datatype::array_payload_element_type(leaves))
                                .map(str::to_owned);
                            let struct_array_element =
                                payload_field_type.as_ref().and_then(|field_type| {
                                    datatype::struct_array_payload_element_type(
                                        field_type,
                                        vc.module_env,
                                    )
                                });
                            if let Some(element_type) = array_element {
                                let Some(array) = datatype::variant_selector_apply(
                                    &sort,
                                    variant_idx,
                                    off,
                                    target,
                                ) else {
                                    continue;
                                };
                                let Some(len) = datatype::variant_selector_apply(
                                    &sort,
                                    variant_idx,
                                    off + 1,
                                    target,
                                ) else {
                                    continue;
                                };
                                if let Pattern::Variable(name) = field_pattern {
                                    super::z3_types::mark_array_shared(vc.ctx, env, name);
                                    env.insert(name.clone(), array.clone());
                                    env.insert(format!("__z3_arr_{name}"), array.clone());
                                    let len_key = super::z3_types::array_len_key(name);
                                    env.insert(len_key.clone(), len.clone());
                                    if let Some(array_ast) = array.as_array() {
                                        let array_key = super::z3_types::register_array_value_len(
                                            env, &array_ast, len,
                                        );
                                        bound.insert(name.clone());
                                        bound.insert(format!("__z3_arr_{name}"));
                                        bound.insert(len_key);
                                        bound.insert(array_key);
                                        vc.local_array_elem_types
                                            .borrow_mut()
                                            .insert(name.clone(), element_type.clone());
                                    }
                                }
                                continue;
                            }
                            if let Some(element_type) = struct_array_element {
                                let Some(leaves) = payload_leaves.as_ref() else {
                                    continue;
                                };
                                let Some(length_index) =
                                    leaves.iter().position(|(_, base)| base == "array_len")
                                else {
                                    continue;
                                };
                                let Some(len) = datatype::variant_selector_apply(
                                    &sort,
                                    variant_idx,
                                    off + length_index,
                                    target,
                                ) else {
                                    continue;
                                };
                                if let (Some(solver), Some(nonnegative)) =
                                    (solver_opt, nonneg_constraint(vc.ctx, &len))
                                {
                                    solver.assert(&is_v.implies(&nonnegative));
                                }
                                let struct_leaves = super::z3_types::struct_array_leaf_types(
                                    &element_type,
                                    vc.module_env,
                                )
                                .unwrap_or_default();
                                for (path, leaf_type) in &struct_leaves {
                                    let Some((leaf_index, (_, leaf_sort))) = leaves
                                        .iter()
                                        .enumerate()
                                        .find(|(_, (name, _))| name == &format!("#{path}"))
                                    else {
                                        continue;
                                    };
                                    let Some(array) = datatype::variant_selector_apply(
                                        &sort,
                                        variant_idx,
                                        off + leaf_index,
                                        target,
                                    ) else {
                                        continue;
                                    };
                                    if let Some(solver) = solver_opt {
                                        let domain = if leaf_sort == &format!("array:{leaf_type}") {
                                            if leaf_type.starts_with("enum:") {
                                                datatype::enum_array_leaf_domain(vc, &array, &len)
                                            } else {
                                                datatype::array_int_leaf_domain(
                                                    vc, &array, &len, leaf_type,
                                                )
                                            }
                                        } else {
                                            None
                                        };
                                        if let Some(domain) = domain {
                                            solver.assert(&is_v.implies(&domain));
                                        }
                                    }
                                    if let Pattern::Variable(name) = field_pattern {
                                        let key =
                                            super::z3_types::struct_array_leaf_key(name, path);
                                        env.insert(key.clone(), array.clone());
                                        bound.insert(key);
                                    }
                                }
                                if let Pattern::Variable(name) = field_pattern {
                                    super::z3_types::mark_array_shared(vc.ctx, env, name);
                                    let carrier_name =
                                        format!("__enum_struct_array_{name}_{}", env.len());
                                    let Ok(carrier) = super::z3_types::z3_array_for_sort(
                                        vc.ctx,
                                        &carrier_name,
                                        ArrayElementSort::Struct,
                                    ) else {
                                        continue;
                                    };
                                    env.insert(name.clone(), carrier.clone().into());
                                    env.insert(format!("__z3_arr_{name}"), carrier.clone().into());
                                    let len_key = super::z3_types::array_len_key(name);
                                    let root_len_key = super::z3_types::array_root_len_key(name);
                                    env.insert(len_key.clone(), len.clone());
                                    env.insert(root_len_key.clone(), len.clone());
                                    super::z3_types::register_array_value_len(env, &carrier, len);
                                    bound.insert(name.clone());
                                    bound.insert(format!("__z3_arr_{name}"));
                                    bound.insert(len_key);
                                    bound.insert(root_len_key);
                                    vc.local_array_elem_types
                                        .borrow_mut()
                                        .insert(name.clone(), element_type);
                                }
                                continue;
                            }
                            let sdef = enum_def.and_then(|edef| {
                                // Over-wide patterns (`V(a, b)` on
                                // `V(i64)`) index past the declared
                                // fields — stay on the scalar path. An
                                // enum-typed field is one inner-datatype
                                // accessor, not a flattened struct, so a
                                // nested variant pattern binds through
                                // the scalar path below.
                                let ft = datatype::payload_field_type_name(
                                    edef.variants[variant_idx].field_types.get(i)?,
                                );
                                let resolved = vc.module_env.resolve_base_type(&ft);
                                if vc.module_env.enums.contains_key(&resolved) {
                                    return None;
                                }
                                vc.module_env.structs.get(&resolved)
                            });
                            if let Some(sdef) = sdef {
                                // Composite payload: bind a fresh struct
                                // handle whose leaves are the selector
                                // applies, under both the handle's decl
                                // name and any bound variable name. Leaf
                                // names come from `field_leaf_types` so a
                                // nested payload lands on the same
                                // `<name>_<path>` keys reads resolve.
                                let leaf_types = enum_def.and_then(|edef| {
                                    datatype::field_leaf_types(
                                        &datatype::payload_field_type_name(
                                            edef.variants[variant_idx].field_types.get(i)?,
                                        ),
                                        vc.module_env,
                                    )
                                });
                                let mut leaves: Vec<(String, Dynamic)> = Vec::with_capacity(width);
                                for j in off..off + width {
                                    let Some(leaf) = datatype::variant_selector_apply(
                                        &sort,
                                        variant_idx,
                                        j,
                                        target,
                                    ) else {
                                        continue;
                                    };
                                    let leaf_name = leaf_types
                                        .as_ref()
                                        .and_then(|lt| lt.get(j - off))
                                        .map(|(n, _)| n.clone())
                                        .unwrap_or_else(|| sdef.fields[j - off].name.clone());
                                    leaves.push((leaf_name, leaf));
                                }
                                if leaves.len() != width {
                                    continue;
                                }
                                let handle = crate::verification::translator::fresh_struct_handle(
                                    vc.ctx, &sdef.name,
                                );
                                let handle_name = handle.decl().name();
                                // The handle carries the declared direct
                                // fields: a struct-typed field projects to a
                                // fresh inner handle so `b.inner` resolves
                                // (e.g. returning the bound `b` as its
                                // struct type) — the flat `inner_x` keys
                                // stay off the handle so projection sees
                                // declared fields only.
                                let leaf_vals: Vec<Dynamic> =
                                    leaves.iter().map(|(_, v)| v.clone()).collect();
                                let direct = match rebuild_payload_direct_fields(
                                    vc.ctx, vc, env, sdef, &leaf_vals,
                                ) {
                                    Some(direct) => direct,
                                    None if leaf_types.as_ref().is_some_and(|types| {
                                        datatype::array_payload_leaf_pairs(types).next().is_some()
                                    }) =>
                                    {
                                        continue;
                                    }
                                    None => leaves.clone(),
                                };
                                crate::verification::translator::bind_struct_fields(
                                    env,
                                    &handle_name,
                                    &direct,
                                );
                                env.insert(
                                    format!("__proj_{}_{}", variant_leaf, i),
                                    handle.clone(),
                                );
                                if let Pattern::Variable(fname) = field_pattern {
                                    let array_length_indices: std::collections::HashSet<usize> =
                                        leaf_types
                                            .as_ref()
                                            .map(|types| {
                                                datatype::array_payload_leaf_pairs(types)
                                                    .map(|(array_index, _)| array_index + 1)
                                                    .collect()
                                            })
                                            .unwrap_or_default();
                                    let struct_leaves: Vec<(String, Dynamic)> = leaves
                                        .iter()
                                        .enumerate()
                                        .filter(|(index, _)| !array_length_indices.contains(index))
                                        .map(|(_, leaf)| leaf.clone())
                                        .collect();
                                    crate::verification::translator::bind_struct_fields(
                                        env,
                                        fname,
                                        &struct_leaves,
                                    );
                                    crate::verification::translator::bind_struct_fields(
                                        env, fname, &direct,
                                    );
                                    env.insert(fname.clone(), handle.clone());
                                    // The bound name shadows any outer
                                    // binding — its field slots
                                    // (`{fname}_{leaf}`,
                                    // `__struct_{fname}_{leaf}`,
                                    // `{fname}_{field}`) are
                                    // arm-local too, so the merge must
                                    // not fold them into the post-match
                                    // env like assignments.
                                    for (field, _) in &direct {
                                        bound.insert(format!("{fname}_{field}"));
                                        bound.insert(
                                            crate::verification::translator::struct_field_key(
                                                fname, field,
                                            ),
                                        );
                                    }
                                    for (index, (leaf, _)) in leaves.iter().enumerate() {
                                        if array_length_indices.contains(&index) {
                                            continue;
                                        }
                                        bound.insert(format!("{fname}_{leaf}"));
                                        bound.insert(
                                            crate::verification::translator::struct_field_key(
                                                fname, leaf,
                                            ),
                                        );
                                    }
                                }
                                continue;
                            }
                            // A slot beyond the constructor's accessor
                            // count (over-wide pattern) has no selector —
                            // bind a fresh projector like the Int-tag
                            // path does instead of panicking.
                            let field_sym =
                                datatype::variant_selector_apply(&sort, variant_idx, off, target)
                                    .unwrap_or_else(|| {
                                        Int::new_const(
                                            vc.ctx,
                                            format!("__proj_{}_{}", variant_leaf, i).as_str(),
                                        )
                                        .into()
                                    });
                            env.insert(format!("__proj_{}_{}", variant_leaf, i), field_sym.clone());
                            match field_pattern {
                                Pattern::Variable(fname) => {
                                    env.insert(fname.clone(), field_sym.clone());
                                    if let Some(edef) = enum_def {
                                        if let Some(target_enum) = edef
                                            .variants
                                            .get(variant_idx)
                                            .and_then(|variant| variant.field_types.get(i))
                                            .and_then(|field_type| {
                                                datatype::recursive_field_target(
                                                    edef,
                                                    field_type,
                                                    vc.module_env,
                                                )
                                            })
                                        {
                                            if let (Some(solver), Some(domain)) = (
                                                solver_opt,
                                                datatype::enum_value_leaf_domain(vc, &field_sym),
                                            ) {
                                                solver.assert(&is_v.implies(&domain));
                                            }
                                            vc.local_enum_types
                                                .borrow_mut()
                                                .insert(fname.clone(), target_enum.name.clone());
                                        }
                                    }
                                    // A payload leaf keeps its declared
                                    // width — `u64`/`u32`/`i32` — so later
                                    // `x < y` / `x - 1` pick the unsigned
                                    // op semantics the LLVM side emits.
                                    // Enum-typed slots resolve to a name
                                    // `fixed_width_int` rejects — skipped.
                                    if let Some((ty, _, _)) = enum_def
                                        .and_then(|ed| ed.variants[variant_idx].field_types.get(i))
                                        .map(|ft| vc.module_env.resolve_base_type(&ft.name))
                                        .and_then(|base| {
                                            crate::verification::translator::expr::fixed_width_int(
                                                &base,
                                            )
                                        })
                                    {
                                        vc.local_int_types.borrow_mut().insert(fname.clone(), ty);
                                    }
                                }
                                Pattern::Variant { .. } => {
                                    pattern_bind_variables(
                                        field_pattern,
                                        &field_sym,
                                        env,
                                        vc,
                                        solver_opt,
                                        None,
                                        bound,
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                    return;
                }
            }
            if let Ok(Some(enum_def)) =
                datatype::resolve_variant_owner(vc, target, variant_name, decl_hint)
            {
                if let Some(variant_def) = enum_def.variants.iter().find(|v| v.name == variant_leaf)
                {
                    for (i, field_pattern) in fields.iter().enumerate() {
                        let proj_name = format!("__proj_{}_{}_{}", enum_def.name, variant_leaf, i);
                        let field_sym: Dynamic = if i < variant_def.field_types.len() {
                            let field_type = &variant_def.field_types[i].name;
                            let base = if *field_type == enum_def.name {
                                "i64".to_string()
                            } else {
                                module_env.resolve_base_type(field_type)
                            };
                            match base.as_str() {
                                "f64" => {
                                    Float::new_const(vc.ctx, proj_name.as_str(), 11, 53).into()
                                }
                                "bool" => Bool::new_const(vc.ctx, proj_name.as_str()).into(),
                                _ => Int::new_const(vc.ctx, proj_name.as_str()).into(),
                            }
                        } else {
                            Int::new_const(vc.ctx, proj_name.as_str()).into()
                        };
                        env.insert(proj_name.clone(), field_sym.clone());

                        // Variable パターン: projector を変数名にもバインド
                        match field_pattern {
                            Pattern::Variable(fname) => {
                                env.insert(fname.clone(), field_sym.clone());
                                // Same declared-width registration as the
                                // Datatype path — an unregistered `u64`
                                // leaf would compare and subtract as
                                // signed `i64`, diverging from LLVM's
                                // unsigned ops on high-bit values.
                                if let Some((ty, _, _)) = variant_def
                                    .field_types
                                    .get(i)
                                    .map(|ft| {
                                        if ft.name == enum_def.name {
                                            "i64".to_string()
                                        } else {
                                            module_env.resolve_base_type(ft.name.as_str())
                                        }
                                    })
                                    .and_then(|base| {
                                        crate::verification::translator::expr::fixed_width_int(
                                            &base,
                                        )
                                    })
                                {
                                    vc.local_int_types.borrow_mut().insert(fname.clone(), ty);
                                }
                            }
                            Pattern::Variant { .. } => {
                                // ネストした Variant: フィールドの宣言型
                                // （再帰 `Self` → 自身の enum 名）を hint に
                                let field_hint: Option<String> =
                                    variant_def.field_types.get(i).and_then(|ft| {
                                        let resolved = if ft.name == enum_def.name {
                                            enum_def.name.clone()
                                        } else {
                                            module_env.resolve_base_type(ft.name.as_str())
                                        };
                                        module_env.get_enum(&resolved).map(|_| resolved)
                                    });
                                pattern_bind_variables(
                                    field_pattern,
                                    &field_sym,
                                    env,
                                    vc,
                                    solver_opt,
                                    field_hint.as_deref(),
                                    bound,
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        Pattern::Wildcard | Pattern::Literal(_) => {}
    }
}

/// アームの Variant パターンから対応する EnumDef を検出する。
/// 最初に解決できた Variant パターンの所属 Enum を返す。複数の Enum が
/// 同名 Variant を持つ場合は match target の宣言型で決定する
/// （`resolve_variant_owner` — HashMap の先見つけ順に依存しない）。
pub(crate) fn detect_enum_from_arms<'a>(
    arms: &[MatchArm],
    vc: &VCtx<'a>,
    target: &Dynamic<'a>,
    decl_hint: Option<&str>,
) -> Option<&'a EnumDef> {
    for arm in arms {
        if let Pattern::Variant { variant_name, .. } = &arm.pattern {
            if let Ok(Some(enum_def)) =
                datatype::resolve_variant_owner(vc, target, variant_name, decl_hint)
            {
                return Some(enum_def);
            }
        }
    }
    None
}

/// Z3 Model から反例の文字列表現を生成する。
/// Enum ドメイン制約が注入されている場合、tag 値からバリアント名+フィールド値を表示する。
pub(crate) fn format_counterexample<'a>(
    model: &z3::Model,
    target: &Dynamic<'a>,
    arms: &[MatchArm],
    env: &Env<'a>,
    vc: &VCtx<'a>,
    decl_hint: Option<&str>,
) -> (String, Option<RaisedCounterexample>) {
    // アームから Enum 定義を特定（ドメイン制約と同じロジック）
    let enum_ctx = detect_enum_from_arms(arms, vc, target, decl_hint);
    let module_env = vc.module_env;

    // ターゲット変数の具体的な値を取得
    if let Some(target_val) = model.eval(target, true) {
        let resolved = if let Some(target_int) = target_val.as_int() {
            let tag_str = format!("{}", target_int);
            if let Ok(tag_val) = tag_str.parse::<i64>() {
                let tag = tag_val as usize;
                enum_ctx
                    .and_then(|enum_def| enum_def.variants.get(tag).map(|_| (enum_def, tag)))
                    .or_else(|| {
                        module_env.enums.values().find_map(|enum_def| {
                            enum_def.variants.get(tag).map(|_| (enum_def, tag))
                        })
                    })
            } else {
                None
            }
        } else if target_val.as_datatype().is_some() {
            datatype_ctor_name(&target_val)
                .and_then(|name| resolve_ctor_variant(vc, enum_ctx, module_env, &target_val, &name))
        } else {
            None
        };
        let type_hint = decl_hint
            .or_else(|| resolved.map(|(enum_def, _)| enum_def.name.as_str()))
            .or_else(|| enum_ctx.map(|enum_def| enum_def.name.as_str()));
        let (rendering, lowering, status) =
            raise_binding(model, "target", &target_val, type_hint, env, module_env);
        let decoded_string = if lowering == "string" {
            decode_model_string(&target_val, &rendering)
        } else {
            None
        };
        let message = if let Some((enum_def, idx)) = resolved {
            format_missing_variant(enum_def, &enum_def.variants[idx], idx as i64)
        } else {
            format!("value = {rendering} -- no matching arm")
        };
        let raised = RaisedCounterexample {
            values: vec![RaisedValue {
                source_name: "target".to_string(),
                solver_name: None,
                rendering,
                lowering,
                source_type: type_hint.map(str::to_string),
                decoded_string,
                status,
            }],
            omitted_solver_symbols: Vec::new(),
        };
        (message, Some(raised))
    } else {
        // 評価に失敗した場合、アームの情報からヒントを生成
        let covered: Vec<String> = arms
            .iter()
            .map(|arm| match &arm.pattern {
                Pattern::Literal(n) => format!("{}", n),
                Pattern::Variant { variant_name, .. } => variant_name.clone(),
                Pattern::Variable(name) => format!("_{} (bind)", name),
                Pattern::Wildcard => "_".to_string(),
            })
            .collect();
        (
            format!(
                "(could not evaluate; covered patterns: [{}])",
                covered.join(", ")
            ),
            None,
        )
    }
}

/// `Enum::Variant(T=?, ...) (tag=N) -- missing from match arms` 形式で表示する。
fn format_missing_variant(edef: &EnumDef, variant: &EnumVariant, tag_val: i64) -> String {
    let fields_display = if variant.fields.is_empty() {
        String::new()
    } else {
        let field_vals: Vec<String> = variant
            .fields
            .iter()
            .map(|field_type| format!("{}=?", field_type))
            .collect();
        format!("({})", field_vals.join(", "))
    };
    format!(
        "{}::{}{} (tag={}) -- missing from match arms",
        edef.name, variant.name, fields_display, tag_val
    )
}

/// モデル値が datatype コンストラクタ適用なら先頭宣言名（= variant 名）を返す。
/// 0 項の `PEP` でも `(Circle 3)` のようなペイロード付きでも decl 名は同じ。
fn datatype_ctor_name(target_val: &Dynamic) -> Option<String> {
    if target_val.kind() != z3::AstKind::App {
        return None;
    }
    Some(target_val.safe_decl().ok()?.name())
}

/// コンストラクタ名（variant 名）から所属 Enum と tag（variants 内 index）を解決する。
/// 1. arms から検出した enum_ctx がその variant を持てばそれを使う。
/// 2. なければ module_env のうちソートがモデル値と一致する Enum
///    （datatype ソート名は enum 名なので、sort 一致 == 名前一致）。
fn resolve_ctor_variant<'a>(
    vc: &VCtx<'a>,
    enum_ctx: Option<&'a EnumDef>,
    module_env: &'a ModuleEnv,
    target_val: &Dynamic,
    variant_name: &str,
) -> Option<(&'a EnumDef, usize)> {
    let target_sort = target_val.as_datatype()?.get_sort();
    // モデル値のソートが正: enum_ctx は arms からの推測なので、同名 variant を
    // 持つ別 enum を誤検出している可能性がある。ソートが一致するときだけ使う。
    if let Some(edef) = enum_ctx {
        if let Some(sort) = datatype::enum_datatype_sort(vc, edef) {
            if sort.sort == target_sort {
                if let Some(idx) = edef.variants.iter().position(|v| v.name == variant_name) {
                    return Some((edef, idx));
                }
            }
        }
    }
    for edef in module_env.enums.values() {
        if let Some(sort) = datatype::enum_datatype_sort(vc, edef) {
            if sort.sort == target_sort {
                if let Some(idx) = edef.variants.iter().position(|v| v.name == variant_name) {
                    return Some((edef, idx));
                }
            }
        }
    }
    None
}
