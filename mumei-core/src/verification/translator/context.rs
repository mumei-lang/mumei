#![allow(unused_imports)]
use super::super::support::*;
use super::super::*;
use super::*;
use crate::lowering::{lower, LoweredType};

pub const DEFAULT_CONSTRAINT_BUDGET: usize = 1000;

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
    pub(crate) profiler: Option<&'a std::cell::RefCell<IncrementalProfiler<'a>>>,
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
