#![allow(unused_imports)]
use super::super::support::*;
use super::super::*;
use super::*;
use crate::lowering::{lower, LoweredType};

pub const DEFAULT_CONSTRAINT_BUDGET: usize = 1000;

/// A lambda-shaped name in `VCtx::local_lambdas`, so an indirect call
/// `f(args)` / `call(f, args)` can resolve instead of failing as an
/// unknown function.
#[derive(Debug, Clone)]
pub(crate) enum LocalLambda<'a> {
    /// A `let`/`assign`-bound lambda (`let f = |a| a + 1`): the call
    /// inlines the body with formal parameters bound to the evaluated
    /// arguments — the call behaves like `let p_i = arg_i in body`.
    Closure {
        /// The `Expr::Lambda` the name was bound to.
        expr: crate::parser::Expr,
        /// The `local_lambdas` contents captured at the binding site: free
        /// lambda-variable names inside `expr`'s body resolve against this
        /// captured scope — not the (possibly rebound) call-site scope — so
        /// `let g = …; let f = |a| g(a); let g = …; f(x)` keeps the `g`
        /// that was live when `f` was defined.
        captured: std::collections::HashMap<String, std::rc::Rc<LocalLambda<'a>>>,
    },
    /// `let h = if c { f } else { g }` where both branch tails resolve to
    /// lambdas: `h(args)` applies as `ite(cond, f(args), g(args))`. `cond`
    /// is evaluated once at the binding site, so a later `c = …` rebind
    /// cannot silently re-pick the branch the call inlines.
    Ite {
        cond: z3::ast::Bool<'a>,
        then_lam: std::rc::Rc<LocalLambda<'a>>,
        else_lam: std::rc::Rc<LocalLambda<'a>>,
    },
    /// Placeholder for a lambda's own parameter while the lambda body is
    /// checked at binding time (`let apply = |f, x| f(x)`): the param's
    /// arity and return type are unknown until a call site supplies a
    /// closure, so a call through it yields a fresh symbolic int. Only
    /// ever present during the bind-time body check — applying a closure
    /// re-binds params that receive lambdas to real `Closure` entries.
    Opaque,
}

/// 検証時に共有するコンテキスト（ctx, module_env を束ねて引数を削減）
pub(crate) struct VCtx<'a> {
    pub(crate) ctx: &'a Context,
    pub(crate) module_env: &'a ModuleEnv,
    /// Phase B call_with_contract: 現在検証中の atom への参照。
    /// CallRef の動的ケース（パラメトリック関数型）で、呼び出し先の関数パラメータに
    /// 宣言された contract(f) 情報を取得するために使用する。
    pub(crate) current_atom: Option<&'a crate::parser::Atom>,
    /// LinearityCtx for ownership/borrowing tracking during body evaluation.
    /// Wrapped in RefCell so that recursive expr_to_z3/stmt_to_z3 calls can
    /// mutate it without changing every call-site signature.
    pub(crate) linearity_ctx: Option<&'a std::cell::RefCell<LinearityCtx>>,
    pub(crate) inferred_invariants: Option<
        &'a std::cell::RefCell<Vec<crate::verification::invariant_inference::InferredInvariant>>,
    >,
    /// EffectCtx for tracking allowed vs used effects during body evaluation.
    pub(crate) effect_ctx: Option<&'a std::cell::RefCell<EffectCtx>>,
    /// Per-atom constraint budget: tracks the number of solver.assert() calls.
    /// When the count exceeds the limit, verification returns an error to
    /// prevent Z3 explosion on pathological inputs.
    pub(crate) constraint_count: Option<&'a std::cell::Cell<usize>>,
    /// Maximum allowed constraint count for this atom.
    pub(crate) constraint_budget: usize,
    /// Flag set to true when Z3 String Sort constraints are added.
    /// When true, the Sort-aware timeout mechanism doubles timeout_ms
    /// to accommodate the higher complexity of string theory solving.
    pub(crate) has_string_constraints: Option<&'a std::cell::Cell<bool>>,
    /// Stack of path conditions accumulated while descending into nested
    /// `if … else …` branches. These are *not* asserted on the persistent
    /// solver — instead, intermediate check sites (loop-invariant base
    /// case / preservation, decreases monotonicity, …) conjoin the stack
    /// with the negated check so that branch-local guards (e.g. the `n > 1`
    /// implied by being in the `else` of `if n <= 1`) participate in the
    /// satisfiability query without leaking into sibling branches.
    pub(crate) path_cond_stack: std::cell::RefCell<Vec<Bool<'a>>>,
    pub(crate) held_resources: std::cell::RefCell<std::collections::HashMap<String, usize>>,
    pub(crate) acquire_counter: std::cell::RefCell<usize>,
    /// Monotonic per-context ID used to namespace loop snapshots during
    /// nested invariant inference probes.
    pub(crate) loop_counter: std::cell::RefCell<usize>,
    pub(crate) profiler: Option<&'a std::cell::RefCell<IncrementalProfiler<'a>>>,
    /// Opt-in IEEE 754 `f64` verification (`--ieee754-f64`). When `true`,
    /// `f64` parameters/literals are encoded as Z3 IEEE 754 binary64 `Float`
    /// and `f64` arithmetic is lowered to the FP theory (round-nearest-even).
    /// When `false` (default), `f64` uses the exact-rational `Real` encoding.
    pub(crate) ieee754_f64: bool,
    /// Opt-in bit-vector `i64` verification (`--bitvec-i64`). When `true`,
    /// `i64` parameters/results are encoded as Z3 `BV(64)`, so `&`/`|`/`^`/
    /// `<<`/`>>` carry real bit semantics and `+`/`-`/`*` wrap in two's
    /// complement. When `false` (default), `i64` uses the unbounded `Int`
    /// encoding.
    pub(crate) bitvec_i64: bool,
    /// Whether `bitvec_i64` comes from the whole-run opt-in rather than from
    /// this atom's own contract. Under the global opt-in every atom — callees
    /// included — is verified in the bit-vector encoding, which is what decides
    /// whether a callee's `ensures` may be imported at a call site.
    pub(crate) bitvec_i64_global: bool,
    /// Shift amounts lowered while no `Solver` was available (contract clauses
    /// and invariants are lowered with `solver_opt = None`), paired with the
    /// path condition in force at that point. `discharge_bv_shift_obligations`
    /// replays them against the atom's solver so that `0 <= n < 64` is enforced
    /// for `requires`/`ensures`/invariant shifts too, not just body shifts.
    pub(crate) bv_shift_obligations: std::cell::RefCell<Vec<(Bool<'a>, BV<'a>)>>,
    /// `(dividend, divisor)` pairs lowered without a solver, like shift
    /// amounts above, so `divisor != 0` and `!(i64::MIN / -1)` are enforced
    /// for contract divisions too, not just body divisions.
    /// `discharge_bv_div_obligations` replays them under each entry's path
    /// condition.
    pub(crate) bv_div_obligations: std::cell::RefCell<Vec<(Bool<'a>, BV<'a>, BV<'a>)>>,
    /// Clause booleans asserted so far during spec validation, in source
    /// order. Deferred obligations are conditioned on the clauses already in
    /// scope so a bound in one conjunct (`result <= 62`) discharges a shift
    /// in a later one (`n >> result`).
    pub(crate) clause_context: std::cell::RefCell<Vec<Bool<'a>>>,
    /// P10-C: per-context cache of Z3 `DatatypeSort`s created for finite,
    /// non-recursive enums. Rebuilding the same-named datatype would yield a
    /// distinct sort and break `_eq`/`ite` between two declarations of the
    /// same enum, so the first build is reused.
    pub(crate) enum_sorts:
        std::cell::RefCell<crate::verification::support::datatype::EnumSortCache<'a>>,
    /// Declared enum type inferred for `let`/`assign`-bound variables
    /// (`let e = Mine::Cons(1)` records `e -> Mine`), so a `match e` on a
    /// local binding resolves variant owners the same way a declared
    /// parameter type does. Entries are removed when the variable is
    /// reassigned to a value with no inferable enum type.
    pub(crate) local_enum_types: std::cell::RefCell<std::collections::HashMap<String, String>>,
    /// Element type name inferred for `let`-bound array literals
    /// (`let a = [1.0, 2.0]` records `a -> "f64"`), so `array_element_sort`
    /// resolves the element type for a local binding the same way a declared
    /// `[T]` parameter type does. Removed when the variable is rebound to a
    /// non-array value.
    pub(crate) local_array_elem_types:
        std::cell::RefCell<std::collections::HashMap<String, String>>,
    /// `let`/`assign`-bound lambdas (`let f = |a| a + 1` records
    /// `f -> LocalLambda`) so `f(args)` / `call(f, args)` inline the body
    /// instead of failing as an unknown function. Entries drop when the
    /// variable is rebound to a non-lambda value and are branch-scoped
    /// the same way `local_enum_types` is. `Rc` wrapping keeps binding
    /// *identity* through map clones — an alias (`let g = f`) shares the
    /// same `Rc`, which is what lets the if/else merge keep a name only
    /// when both sides left the same binding in place (`Rc::ptr_eq`).
    pub(crate) local_lambdas:
        std::cell::RefCell<std::collections::HashMap<String, std::rc::Rc<LocalLambda<'a>>>>,
    /// Len symbol minted for an array-returning call, keyed by the result
    /// array's raw `Z3_ast` pointer. A callee's `ensures: len(result) == k`
    /// asserts on that symbol; the map lets `let t = f(..)` bind `len_t` to
    /// the *same* symbol so the guarantee reaches the caller instead of a
    /// fresh unconstrained `len_t`. The stored `Dynamic` keeps the key ast
    /// alive (Z3 refcount) so a freed pointer can never alias a new const.
    pub(crate) call_result_lens:
        std::cell::RefCell<std::collections::HashMap<usize, (Dynamic<'a>, Dynamic<'a>)>>,
}

impl<'a> VCtx<'a> {
    /// Conjunction of the current branch path conditions (or `true` if
    /// no enclosing `if/else` has narrowed the path). Used at intermediate
    /// check sites (see `path_cond_stack` doc) so that branch guards
    /// participate in the SAT query.
    pub(crate) fn path_cond_conj(&self) -> Bool<'a> {
        let stack = self.path_cond_stack.borrow();
        if stack.is_empty() {
            Bool::from_bool(self.ctx, true)
        } else {
            let refs: Vec<&Bool<'a>> = stack.iter().collect();
            Bool::and(self.ctx, &refs)
        }
    }
}

// =============================================================================
// 線形性チェック（Linear Types / Ownership Tracking）
// =============================================================================
//
// NOTE (Plan 19 — Phase 4c complete): The primary ownership/move analysis has
// been migrated to MIR-based MoveAnalysis (mumei-core/src/mir_analysis.rs).  Phase 1h in
// verify() now runs forward dataflow move analysis on the MIR CFG and reports
// UseAfterMove, DoubleMove, and ConflictingMerge as hard errors.
//
// LinearityCtx is retained as a secondary Z3-integrated check for:
// - Borrow tracking at call sites (ref / ref mut parameter handling)
// - Consume tracking within Z3 symbolic execution (ensures __alive_ bools)
// - Violation accumulation for the Phase 5b linearity report
//
// Future: Once MIR borrow tracking is implemented (Phase 5), LinearityCtx
// can be fully removed.
//
pub(crate) fn check_constraint_budget(vc: &VCtx, atom_name: &str) -> MumeiResult<()> {
    if let Some(cell) = vc.constraint_count {
        let new_count = cell.get() + 1;
        cell.set(new_count);
        if new_count > vc.constraint_budget {
            return Err(MumeiError::verification(format!(
                "Constraint budget exceeded for atom '{}': {} constraints (limit: {})",
                atom_name, new_count, vc.constraint_budget
            )));
        }
    }
    Ok(())
}

pub(crate) fn profile_solver_assertion(
    vc: &VCtx<'_>,
    constraint_id: &str,
    source_location: Option<String>,
) {
    if let Some(profiler) = vc.profiler {
        profiler
            .borrow_mut()
            .profile_assertion(constraint_id, source_location);
    }
}

pub(crate) fn profiler_checkpoint(vc: &VCtx<'_>) -> Option<usize> {
    vc.profiler
        .map(|profiler| profiler.borrow_mut().begin_check())
}

pub(crate) fn profile_solver_check(vc: &VCtx<'_>, start_index: Option<usize>) {
    if let (Some(profiler), Some(start_index)) = (vc.profiler, start_index) {
        profiler.borrow_mut().end_check(start_index);
    }
}
