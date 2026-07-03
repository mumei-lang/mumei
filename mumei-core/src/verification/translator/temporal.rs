#![allow(unused_imports)]
use super::super::support::*;
use super::super::*;
use super::*;
use crate::lowering::{lower, LoweredType};
use z3::{FuncDecl, Sort};

pub(crate) fn is_temporal_witness_predicate(name: &str) -> bool {
    matches!(name, "server_bound" | "server_listening" | "request_live")
}

fn temporal_witness_app<'a>(ctx: &'a Context, predicate: &str, handle: &Int<'a>) -> Bool<'a> {
    let int_sort = Sort::int(ctx);
    let bool_sort = Sort::bool(ctx);
    let decl = FuncDecl::new(ctx, predicate, &[&int_sort], &bool_sort);
    decl.apply(&[handle as &dyn Ast])
        .as_bool()
        .unwrap_or_else(|| {
            Bool::new_const(
                ctx,
                format!("__{}_{}", predicate, handle)
                    .replace(' ', "_")
                    .as_str(),
            )
        })
}

pub(crate) fn temporal_witness_call_to_z3<'a>(
    vc: &VCtx<'a>,
    predicate: &str,
    args: &[Expr],
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> DynResult<'a> {
    if args.len() != 1 {
        return Err(MumeiError::verification(format!(
            "{}() requires exactly one handle argument",
            predicate
        )));
    }

    let handle = expr_to_z3(vc, &args[0], env, solver_opt)?
        .as_int()
        .ok_or_else(|| MumeiError::type_error(format!("{}() handle must be integer", predicate)))?;
    let witness = temporal_witness_app(vc.ctx, predicate, &handle);
    if let Some(solver) = solver_opt {
        solver.assert(&witness.implies(&handle.gt(&Int::from_i64(vc.ctx, 0))));
    }
    Ok(witness.into())
}

pub(crate) fn assert_temporal_effect_transition<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    callee_name: &str,
    arg_vals: &[Dynamic<'a>],
    result_z3: &Dynamic<'a>,
) -> MumeiResult<()> {
    match callee_name {
        "http_server_bind" | "bind_server" => {
            if let Some(result) = result_z3.as_int() {
                let bound = temporal_witness_app(vc.ctx, "server_bound", &result);
                // TODO(http_server): Once the Rust FFI layer exposes a no-zero-result
                // guarantee for valid bind/accept inputs, tighten std/http_server.mm
                // contracts back to `result > 0 && <witness>(result)`.
                solver.assert(&result.gt(&Int::from_i64(vc.ctx, 0)).implies(&bound));
            }
        }
        "http_server_listen" | "listen_server" => {
            if let Some(handle) = arg_vals.first().and_then(Dynamic::as_int) {
                let bound = temporal_witness_app(vc.ctx, "server_bound", &handle);
                let listening = temporal_witness_app(vc.ctx, "server_listening", &handle);
                solver.assert(&bound.implies(&listening));
            }
        }
        "http_server_accept" | "accept_request" => {
            if let (Some(handle), Some(result)) = (
                arg_vals.first().and_then(Dynamic::as_int),
                result_z3.as_int(),
            ) {
                let listening = temporal_witness_app(vc.ctx, "server_listening", &handle);
                let live = temporal_witness_app(vc.ctx, "request_live", &result);
                let successful_accept =
                    Bool::and(vc.ctx, &[&listening, &result.gt(&Int::from_i64(vc.ctx, 0))]);
                solver.assert(&successful_accept.implies(&live));
            }
        }
        "http_server_respond" | "send_response" => {}
        _ => {}
    }
    Ok(())
}
