// =============================================================================
// MIR Analysis: Liveness Analysis + Drop Insertion + Move Analysis
// =============================================================================
// Implements:
//   1. Backward dataflow liveness analysis on MIR CFG, then inserts
//      MirStatement::Drop at points where variables become dead.
//   2. Forward dataflow move analysis to detect use-after-move, double-move,
//      and conflicting merge at branch join points.
//
// Pipeline: compute_gen_kill → compute_liveness → insert_drops
//           analyze_moves (forward dataflow for ownership tracking)
//
// Plan 19 (Phase 4c complete): This module is now the primary ownership/move
// analysis engine, replacing the HIR-level LinearityCtx for move detection.
// verification.rs Phase 1h calls analyze_moves() and reports violations as
// hard errors. LinearityCtx is retained only for Z3-level borrow tracking.
// =============================================================================

mod gen_kill;
mod liveness;
mod move_analysis;
mod temporal_effects;

#[cfg(test)]
use crate::mir::{Local, MirBody, MirStatement, Movability, Operand, Place, Rvalue};
#[cfg(test)]
use std::collections::HashMap;

pub use gen_kill::{compute_gen_kill, GenKill};
pub use liveness::{compute_liveness, insert_drops, LivenessResult};
pub use move_analysis::{
    analyze_moves, MergeConflict, MirLinearityState, MoveAnalysisResult, MoveViolation,
    MoveViolationKind,
};
pub use temporal_effects::{
    analyze_temporal_effects, analyze_temporal_effects_with_contracts, AtomEffectContract,
    EffectStateMachine, EffectStateMap, TemporalEffectResult, TemporalEffectViolation,
    TemporalViolationKind, MAX_EFFECT_STATES,
};

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::lower_atom_to_hir;
    use crate::mir::{lower_hir_to_mir, BasicBlock, LocalDecl, MirConstant, Terminator};
    use crate::parser::{self, Atom, Param, Span, TrustLevel};

    /// Helper: create a minimal Atom with given body expression and params.
    fn make_atom(name: &str, params: Vec<Param>, body_expr: &str) -> Atom {
        Atom {
            name: name.to_string(),
            type_params: vec![],
            where_bounds: vec![],
            params,
            trace_id: None,
            spec_metadata: std::collections::HashMap::new(),
            requires: "true".to_string(),
            forall_constraints: vec![],
            ensures: "true".to_string(),
            body_expr: body_expr.to_string(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: None,
            span: Span::new("", 1, 1, 0),
            effect_pre: std::collections::HashMap::new(),
            effect_post: std::collections::HashMap::new(),
        }
    }

    fn make_param(name: &str, ty: &str) -> Param {
        Param {
            name: name.to_string(),
            type_name: Some(ty.to_string()),
            type_ref: Some(parser::parse_type_ref(ty)),
            is_ref: false,
            is_ref_mut: false,
            fn_contract_requires: None,
            fn_contract_ensures: None,
        }
    }

    // =========================================================================
    // Test: predecessors/successors (CFG utilities)
    // =========================================================================

    #[test]
    fn test_successors_predecessors() {
        let atom = make_atom(
            "branch",
            vec![make_param("x", "Int")],
            "if x > 0 { x + 1 } else { x - 1 }",
        );
        let hir = lower_atom_to_hir(&atom);
        let mir = lower_hir_to_mir(&hir);

        let succs = mir.successors();
        let preds = mir.predecessors();

        // Every block should have a successor entry
        for block in &mir.blocks {
            assert!(succs.contains_key(&block.id));
        }

        // Every block should have a predecessor entry
        for block in &mir.blocks {
            assert!(preds.contains_key(&block.id));
        }

        // The entry block (0) should have no predecessors from outside
        // (unless there's a loop, which this simple if/else doesn't have)
        // Verify that if a block B has successor S, then S has B as predecessor
        for (block_id, block_succs) in &succs {
            for succ in block_succs {
                let succ_preds = preds.get(succ).unwrap();
                assert!(
                    succ_preds.contains(block_id),
                    "Block {} should be a predecessor of block {}",
                    block_id,
                    succ
                );
            }
        }
    }

    // =========================================================================
    // Test: gen/kill computation
    // =========================================================================

    #[test]
    fn test_gen_kill_basic() {
        // Manually construct a block: assign tmp = a + b
        let block = BasicBlock {
            id: 0,
            statements: vec![MirStatement::Assign(
                Place::Local(Local(2)),
                Rvalue::BinaryOp(
                    crate::parser::Op::Add,
                    Operand::Place(Place::Local(Local(0))),
                    Operand::Place(Place::Local(Local(1))),
                ),
            )],
            terminator: Terminator::Return(Operand::Place(Place::Local(Local(2)))),
        };

        let gk = compute_gen_kill(&block);

        // gen should contain Local(0) and Local(1) (used in BinaryOp)
        assert!(gk.gen.contains(&Local(0)));
        assert!(gk.gen.contains(&Local(1)));
        // kill should contain Local(2) (assigned)
        assert!(gk.kill.contains(&Local(2)));
        // gen should NOT contain Local(2) since it's defined before use in terminator
        // Actually Local(2) IS used in the Return terminator, but it's also killed,
        // so it's already in kill and the terminator use doesn't add to gen
    }

    // =========================================================================
    // Test: liveness analysis
    // =========================================================================

    #[test]
    fn test_liveness_simple() {
        // atom f(x: Int, y: Int) body: { let z = x + 1; z }
        // x should be live at entry, dead after z = x + 1
        // y should be dead throughout (unused)
        let atom = make_atom(
            "f",
            vec![make_param("x", "Int"), make_param("y", "Int")],
            "{ let z = x + 1; z }",
        );
        let hir = lower_atom_to_hir(&atom);
        let mir = lower_hir_to_mir(&hir);
        let liveness = compute_liveness(&mir);

        // Verify liveness result has entries for all blocks
        for block in &mir.blocks {
            assert!(liveness.live_in.contains_key(&block.id));
            assert!(liveness.live_out.contains_key(&block.id));
        }
    }

    // =========================================================================
    // Test: straight-line code — x dead after y = x + 1
    // =========================================================================

    #[test]
    fn test_drop_insertion_straight_line() {
        // atom f(a: Int, b: Int) body: { let y = a + 1; y }
        // After computing y = a + 1, a is dead (and b is never used)
        let atom = make_atom(
            "f",
            vec![make_param("a", "Int"), make_param("b", "Int")],
            "{ let y = a + 1; y }",
        );
        let hir = lower_atom_to_hir(&atom);
        let mut mir = lower_hir_to_mir(&hir);
        let liveness = compute_liveness(&mir);
        insert_drops(&mut mir, &liveness);

        // Check that at least one Drop statement exists in the MIR
        let has_drop = mir.blocks.iter().any(|b| {
            b.statements
                .iter()
                .any(|s| matches!(s, MirStatement::Drop(_)))
        });
        assert!(has_drop, "Should have at least one Drop statement");
    }

    // =========================================================================
    // Test: if/else branch — different drop paths
    // =========================================================================

    #[test]
    fn test_drop_insertion_if_else() {
        // atom f(cond: Int, x: Int) body: if cond > 0 { x + 1 } else { 42 }
        // x is used only in the then-branch; should be dropped in else-branch
        let atom = make_atom(
            "f",
            vec![make_param("cond", "Int"), make_param("x", "Int")],
            "if cond > 0 { x + 1 } else { 42 }",
        );
        let hir = lower_atom_to_hir(&atom);
        let mut mir = lower_hir_to_mir(&hir);
        let liveness = compute_liveness(&mir);
        insert_drops(&mut mir, &liveness);

        // The MIR should have Drop statements in some blocks
        let drop_count: usize = mir
            .blocks
            .iter()
            .map(|b| {
                b.statements
                    .iter()
                    .filter(|s| matches!(s, MirStatement::Drop(_)))
                    .count()
            })
            .sum();
        // At least some drops should be present (parameters become dead)
        assert!(
            drop_count > 0,
            "Should have Drop statements in if/else branches"
        );
    }

    // =========================================================================
    // Test: while loop — loop variable drop after loop
    // =========================================================================

    #[test]
    fn test_drop_insertion_while_loop() {
        // Mumei requires 'invariant' for while loops
        // atom f(n: Int) body: { let i = 0; while i < n invariant: i >= 0 { let i = i + 1; i }; i }
        let mut atom = make_atom(
            "f",
            vec![make_param("n", "Int")],
            "{ let i = 0; while i < n invariant: i >= 0 { let i = i + 1; i }; i }",
        );
        atom.invariant = Some("i >= 0".to_string());
        let hir = lower_atom_to_hir(&atom);
        let mut mir = lower_hir_to_mir(&hir);
        let liveness = compute_liveness(&mir);
        insert_drops(&mut mir, &liveness);

        // Verify the MIR is valid after drop insertion
        assert!(!mir.blocks.is_empty());
    }

    // =========================================================================
    // Test: function call — argument dropped after call if unused
    // =========================================================================

    #[test]
    fn test_drop_insertion_after_call() {
        // atom f(x: Int) body: { let r = add(x, 1); r }
        // x should be dropped after the call to add
        let atom = make_atom(
            "f",
            vec![make_param("x", "Int")],
            "{ let r = add(x, 1); r }",
        );
        let hir = lower_atom_to_hir(&atom);
        let mut mir = lower_hir_to_mir(&hir);
        let liveness = compute_liveness(&mir);
        insert_drops(&mut mir, &liveness);

        // Should have at least one Drop for x after the call
        let has_drop = mir.blocks.iter().any(|b| {
            b.statements
                .iter()
                .any(|s| matches!(s, MirStatement::Drop(_)))
        });
        assert!(has_drop, "Should have Drop for x after call");
    }

    // =========================================================================
    // Test: gen/kill with terminator operands
    // =========================================================================

    #[test]
    fn test_gen_kill_terminator() {
        // Block with no statements but Return(Local(3))
        let block = BasicBlock {
            id: 0,
            statements: vec![],
            terminator: Terminator::Return(Operand::Place(Place::Local(Local(3)))),
        };

        let gk = compute_gen_kill(&block);
        assert!(gk.gen.contains(&Local(3)));
        assert!(gk.kill.is_empty());
    }

    // =========================================================================
    // Test: liveness with multiple blocks
    // =========================================================================

    #[test]
    fn test_liveness_multiple_blocks() {
        // Manually construct a simple two-block MIR:
        //   Block 0: assign tmp = Local(0); goto Block 1
        //   Block 1: return tmp
        let body = MirBody {
            name: "test".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("x".to_string()),
                    ty: Some("Int".to_string()),
                    movability: Movability::Copy,
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("tmp".to_string()),
                    ty: Some("Int".to_string()),
                    movability: Movability::Copy,
                },
            ],
            blocks: vec![
                BasicBlock {
                    id: 0,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(1)),
                        Rvalue::Use(Operand::Place(Place::Local(Local(0)))),
                    )],
                    terminator: Terminator::Goto(1),
                },
                BasicBlock {
                    id: 1,
                    statements: vec![],
                    terminator: Terminator::Return(Operand::Place(Place::Local(Local(1)))),
                },
            ],
            entry_block: 0,
        };

        let liveness = compute_liveness(&body);

        // Block 1: live_in should contain Local(1) (used in Return)
        assert!(liveness.live_in[&1].contains(&Local(1)));

        // Block 0: live_in should contain Local(0) (used to define Local(1))
        assert!(liveness.live_in[&0].contains(&Local(0)));

        // Block 0: live_out should contain Local(1) (live_in of successor Block 1)
        assert!(liveness.live_out[&0].contains(&Local(1)));
    }

    // =========================================================================
    // Move Analysis Tests
    // =========================================================================

    #[test]
    fn test_move_analysis_normal_case() {
        // atom f(x: Int) body: { let y = x + 1; y }
        // No move violations expected — x is used (read) but not moved
        let atom = make_atom("f", vec![make_param("x", "Int")], "{ let y = x + 1; y }");
        let hir = lower_atom_to_hir(&atom);
        let mir = lower_hir_to_mir(&hir);
        let result = analyze_moves(&mir);

        // Should have entry/exit states for all blocks
        for block in &mir.blocks {
            assert!(result.entry_states.contains_key(&block.id));
            assert!(result.exit_states.contains_key(&block.id));
        }

        // No use-after-move or double-move violations expected
        let critical_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| {
                v.kind == MoveViolationKind::UseAfterMove || v.kind == MoveViolationKind::DoubleMove
            })
            .collect();
        assert!(
            critical_violations.is_empty(),
            "Normal code should have no critical move violations, got: {:?}",
            critical_violations
                .iter()
                .map(|v| format!("{} at block {}", v.kind, v.block_id))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_move_analysis_use_after_move() {
        // Manually construct MIR for: let y = move x; use x (should detect UseAfterMove)
        // Block 0: y = Use(x); return x
        // The second use of x after it has been consumed should be a violation
        let body = MirBody {
            name: "use_after_move".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("x".to_string()),
                    ty: Some("MyStruct".to_string()),
                    movability: Movability::Move,
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("y".to_string()),
                    ty: Some("MyStruct".to_string()),
                    movability: Movability::Move,
                },
            ],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![
                    // y = move x (Use of Place is a move)
                    MirStatement::Assign(
                        Place::Local(Local(1)),
                        Rvalue::Use(Operand::Place(Place::Local(Local(0)))),
                    ),
                ],
                // Return x — but x was already moved!
                terminator: Terminator::Return(Operand::Place(Place::Local(Local(0)))),
            }],
            entry_block: 0,
        };

        let result = analyze_moves(&body);

        // Should detect UseAfterMove for Local(0) in block 0's terminator
        let uam_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.kind == MoveViolationKind::UseAfterMove && v.local == Local(0))
            .collect();
        assert!(
            !uam_violations.is_empty(),
            "Should detect use-after-move for x after move to y"
        );
    }

    #[test]
    fn test_move_analysis_double_move() {
        // Block 0: y = move x; z = move x; return z
        // The second move of x should be a DoubleMove
        let body = MirBody {
            name: "double_move".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("x".to_string()),
                    ty: Some("MyStruct".to_string()),
                    movability: Movability::Move,
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("y".to_string()),
                    ty: Some("MyStruct".to_string()),
                    movability: Movability::Move,
                },
                LocalDecl {
                    local: Local(2),
                    name: Some("z".to_string()),
                    ty: Some("MyStruct".to_string()),
                    movability: Movability::Move,
                },
            ],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![
                    // y = move x
                    MirStatement::Assign(
                        Place::Local(Local(1)),
                        Rvalue::Use(Operand::Place(Place::Local(Local(0)))),
                    ),
                    // z = move x (double move!)
                    MirStatement::Assign(
                        Place::Local(Local(2)),
                        Rvalue::Use(Operand::Place(Place::Local(Local(0)))),
                    ),
                ],
                terminator: Terminator::Return(Operand::Place(Place::Local(Local(2)))),
            }],
            entry_block: 0,
        };

        let result = analyze_moves(&body);

        // Should detect DoubleMove for Local(0)
        let dm_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.kind == MoveViolationKind::DoubleMove && v.local == Local(0))
            .collect();
        assert!(!dm_violations.is_empty(), "Should detect double move of x");
    }

    #[test]
    fn test_move_analysis_conflicting_merge() {
        // if/else where one branch moves x and the other doesn't
        // Block 0: SwitchInt(cond) → then=1, else=2
        // Block 1: y = move x; goto 3
        // Block 2: goto 3 (x is alive here)
        // Block 3: return 0
        // At block 3, x is consumed from path 1 but alive from path 2 → ConflictingMerge
        let body = MirBody {
            name: "conflicting_merge".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("cond".to_string()),
                    ty: Some("Int".to_string()),
                    movability: Movability::Copy,
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("x".to_string()),
                    ty: Some("MyStruct".to_string()),
                    movability: Movability::Move,
                },
                LocalDecl {
                    local: Local(2),
                    name: Some("y".to_string()),
                    ty: Some("MyStruct".to_string()),
                    movability: Movability::Move,
                },
            ],
            blocks: vec![
                BasicBlock {
                    id: 0,
                    statements: vec![],
                    terminator: Terminator::SwitchInt {
                        discr: Operand::Place(Place::Local(Local(0))),
                        targets: vec![(1, 1)],
                        otherwise: 2,
                    },
                },
                BasicBlock {
                    id: 1,
                    statements: vec![
                        // y = move x (x is consumed on this path)
                        MirStatement::Assign(
                            Place::Local(Local(2)),
                            Rvalue::Use(Operand::Place(Place::Local(Local(1)))),
                        ),
                    ],
                    terminator: Terminator::Goto(3),
                },
                BasicBlock {
                    id: 2,
                    statements: vec![],
                    // x is alive on this path
                    terminator: Terminator::Goto(3),
                },
                BasicBlock {
                    id: 3,
                    statements: vec![],
                    terminator: Terminator::Return(Operand::Constant(
                        crate::mir::MirConstant::Int(0),
                    )),
                },
            ],
            entry_block: 0,
        };

        let result = analyze_moves(&body);

        // Should detect ConflictingMerge for Local(1) at block 3
        let cm_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.kind == MoveViolationKind::ConflictingMerge && v.local == Local(1))
            .collect();
        assert!(
            !cm_violations.is_empty(),
            "Should detect conflicting merge for x at join point"
        );
    }

    #[test]
    fn test_move_analysis_loop_move() {
        // While loop where move happens inside the loop body.
        // On 2nd iteration, the local is already consumed → violation detected.
        //
        // Block 0: goto 1
        // Block 1: SwitchInt(cond) → body=2, after=3
        // Block 2: y = move x; goto 1
        // Block 3: return 0
        let body = MirBody {
            name: "loop_move".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("cond".to_string()),
                    ty: Some("Int".to_string()),
                    movability: Movability::Copy,
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("x".to_string()),
                    ty: Some("MyStruct".to_string()),
                    movability: Movability::Move,
                },
                LocalDecl {
                    local: Local(2),
                    name: Some("y".to_string()),
                    ty: Some("MyStruct".to_string()),
                    movability: Movability::Move,
                },
            ],
            blocks: vec![
                BasicBlock {
                    id: 0,
                    statements: vec![],
                    terminator: Terminator::Goto(1),
                },
                BasicBlock {
                    id: 1,
                    statements: vec![],
                    terminator: Terminator::SwitchInt {
                        discr: Operand::Place(Place::Local(Local(0))),
                        targets: vec![(1, 2)],
                        otherwise: 3,
                    },
                },
                BasicBlock {
                    id: 2,
                    statements: vec![
                        // y = move x (on second iteration, x is already consumed)
                        MirStatement::Assign(
                            Place::Local(Local(2)),
                            Rvalue::Use(Operand::Place(Place::Local(Local(1)))),
                        ),
                    ],
                    terminator: Terminator::Goto(1),
                },
                BasicBlock {
                    id: 3,
                    statements: vec![],
                    terminator: Terminator::Return(Operand::Constant(
                        crate::mir::MirConstant::Int(0),
                    )),
                },
            ],
            entry_block: 0,
        };

        let result = analyze_moves(&body);

        // On second iteration, the loop header (block 1) receives a state where x is consumed
        // from block 2's exit. This should cause a ConflictingMerge at block 1 (x alive from
        // block 0, consumed from block 2), and potentially DoubleMove in block 2 on re-visit.
        let loop_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.local == Local(1))
            .collect();
        assert!(
            !loop_violations.is_empty(),
            "Should detect move violation for x inside while loop on second iteration"
        );
    }

    #[test]
    fn test_move_analysis_function_call_consume() {
        // Block 0: result = call callee(x); return x
        // x is read in the call arguments, then used again in return → UseAfterMove
        // (Since all Rvalue::Use(Operand::Place) are treated as moves)
        let body = MirBody {
            name: "call_consume".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("x".to_string()),
                    ty: Some("Int".to_string()),
                    movability: Movability::Copy,
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("result".to_string()),
                    ty: Some("Int".to_string()),
                    movability: Movability::Copy,
                },
            ],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![
                    // result = callee(x)
                    MirStatement::Assign(
                        Place::Local(Local(1)),
                        Rvalue::Call {
                            func: "callee".to_string(),
                            args: vec![Operand::Place(Place::Local(Local(0)))],
                        },
                    ),
                ],
                // Return x — x was passed to callee (read access, not a move via Call)
                // Call args are read, not moved, so x is still alive
                terminator: Terminator::Return(Operand::Place(Place::Local(Local(0)))),
            }],
            entry_block: 0,
        };

        let result = analyze_moves(&body);

        // Call args are read access (not moves), so x should still be alive
        // No UseAfterMove expected since Call doesn't consume arguments
        let uam_for_x: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.kind == MoveViolationKind::UseAfterMove && v.local == Local(0))
            .collect();
        assert!(
            uam_for_x.is_empty(),
            "Call arguments are read access, not moves — x should still be alive"
        );
    }

    #[test]
    fn test_copy_type_reuse_no_violation() {
        // Copy types (Int) can be used multiple times without move violations.
        // Block 0: y = Use(x); z = Use(x); return z
        // x is Copy, so both uses are valid (no consume).
        let body = MirBody {
            name: "copy_reuse".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("x".to_string()),
                    ty: Some("Int".to_string()),
                    movability: Movability::Copy,
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("y".to_string()),
                    ty: Some("Int".to_string()),
                    movability: Movability::Copy,
                },
                LocalDecl {
                    local: Local(2),
                    name: Some("z".to_string()),
                    ty: Some("Int".to_string()),
                    movability: Movability::Copy,
                },
            ],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![
                    MirStatement::Assign(
                        Place::Local(Local(1)),
                        Rvalue::Use(Operand::Place(Place::Local(Local(0)))),
                    ),
                    MirStatement::Assign(
                        Place::Local(Local(2)),
                        Rvalue::Use(Operand::Place(Place::Local(Local(0)))),
                    ),
                ],
                terminator: Terminator::Return(Operand::Place(Place::Local(Local(2)))),
            }],
            entry_block: 0,
        };

        let result = analyze_moves(&body);
        let violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.local == Local(0))
            .collect();
        assert!(
            violations.is_empty(),
            "Copy type Int should not produce move violations, got: {:?}",
            violations
        );
    }

    #[test]
    fn test_move_type_use_after_move_detected() {
        // Move types (MyStruct) should produce UseAfterMove when used after being moved.
        // Block 0: y = Use(x); return x
        // x is Move, so the return after move should be a violation.
        let body = MirBody {
            name: "move_uam".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("x".to_string()),
                    ty: Some("MyStruct".to_string()),
                    movability: Movability::Move,
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("y".to_string()),
                    ty: Some("MyStruct".to_string()),
                    movability: Movability::Move,
                },
            ],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![MirStatement::Assign(
                    Place::Local(Local(1)),
                    Rvalue::Use(Operand::Place(Place::Local(Local(0)))),
                )],
                terminator: Terminator::Return(Operand::Place(Place::Local(Local(0)))),
            }],
            entry_block: 0,
        };

        let result = analyze_moves(&body);
        let uam: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.kind == MoveViolationKind::UseAfterMove && v.local == Local(0))
            .collect();
        assert!(
            !uam.is_empty(),
            "Move type MyStruct should produce UseAfterMove"
        );
    }

    #[test]
    fn test_mir_linearity_state_basic() {
        let locals = vec![
            LocalDecl {
                local: Local(0),
                name: Some("a".to_string()),
                ty: None,
                movability: Movability::Move,
            },
            LocalDecl {
                local: Local(1),
                name: Some("b".to_string()),
                ty: None,
                movability: Movability::Move,
            },
        ];

        let mut state = MirLinearityState::init_all_alive(&locals);
        assert!(state.status[&Local(0)]);
        assert!(state.status[&Local(1)]);

        // Consume a
        assert!(state.consume(&Local(0)).is_ok());
        assert!(!state.status[&Local(0)]);

        // Check alive: a should be consumed, b should be alive
        assert!(state.check_alive(&Local(0)).is_err());
        assert!(state.check_alive(&Local(1)).is_ok());

        // Double consume should fail
        assert!(state.consume(&Local(0)).is_err());
    }

    #[test]
    fn test_mir_linearity_state_merge() {
        let mut state1 = MirLinearityState::new();
        state1.status.insert(Local(0), true);
        state1.status.insert(Local(1), false); // consumed

        let mut state2 = MirLinearityState::new();
        state2.status.insert(Local(0), false); // consumed
        state2.status.insert(Local(1), false); // consumed

        let (merged, conflicts) = state1.merge(&state2);

        // Local(0): alive vs consumed → conflict
        assert!(
            conflicts.iter().any(|c| c.local == Local(0)),
            "Should have conflict for Local(0)"
        );
        // Local(1): both consumed → no conflict
        assert!(
            !conflicts.iter().any(|c| c.local == Local(1)),
            "Should not have conflict for Local(1)"
        );
        // Merged: Local(0) should be consumed (conservative)
        assert!(!merged.status[&Local(0)]);
        // Merged: Local(1) should be consumed
        assert!(!merged.status[&Local(1)]);
    }

    // =========================================================================
    // Temporal Effect Tests
    // =========================================================================

    /// Helper: create a File effect EffectDef with states [Closed, Open].
    fn make_file_effect_def() -> crate::parser::EffectDef {
        use crate::parser::EffectTransition;
        crate::parser::EffectDef {
            name: "File".to_string(),
            params: vec![],
            constraint: None,
            includes: vec![],
            refinement: None,
            parent: vec![],
            span: Span::default(),
            states: vec!["Closed".to_string(), "Open".to_string()],
            transitions: vec![
                EffectTransition {
                    operation: "open".to_string(),
                    from_state: "Closed".to_string(),
                    to_state: "Open".to_string(),
                },
                EffectTransition {
                    operation: "write".to_string(),
                    from_state: "Open".to_string(),
                    to_state: "Open".to_string(),
                },
                EffectTransition {
                    operation: "read".to_string(),
                    from_state: "Open".to_string(),
                    to_state: "Open".to_string(),
                },
                EffectTransition {
                    operation: "close".to_string(),
                    from_state: "Open".to_string(),
                    to_state: "Closed".to_string(),
                },
            ],
            initial_state: Some("Closed".to_string()),
        }
    }

    #[test]
    fn test_effect_state_machine_construction() {
        let def = make_file_effect_def();
        let sm = EffectStateMachine::from_effect_def(&def);
        assert!(sm.is_some());
        let sm = sm.unwrap();
        assert_eq!(sm.effect_name, "File");
        assert_eq!(sm.states.len(), 2);
        assert_eq!(sm.initial_state, "Closed");
        assert_eq!(sm.transitions.len(), 4);
    }

    #[test]
    fn test_effect_state_machine_stateless_returns_none() {
        let def = crate::parser::EffectDef {
            name: "Log".to_string(),
            params: vec![],
            constraint: None,
            includes: vec![],
            refinement: None,
            parent: vec![],
            span: Span::default(),
            states: vec![],
            transitions: vec![],
            initial_state: None,
        };
        assert!(EffectStateMachine::from_effect_def(&def).is_none());
    }

    #[test]
    fn test_effect_state_machine_too_many_states() {
        let def = crate::parser::EffectDef {
            name: "TooMany".to_string(),
            params: vec![],
            constraint: None,
            includes: vec![],
            refinement: None,
            parent: vec![],
            span: Span::default(),
            states: (0..9).map(|i| format!("S{}", i)).collect(),
            transitions: vec![],
            initial_state: Some("S0".to_string()),
        };
        assert!(EffectStateMachine::from_effect_def(&def).is_none());
    }

    #[test]
    fn test_effect_state_machine_invalid_initial_state() {
        // initial state "C" is not in states [A, B] → should return None
        let def = crate::parser::EffectDef {
            name: "Bad".to_string(),
            params: vec![],
            constraint: None,
            includes: vec![],
            refinement: None,
            parent: vec![],
            span: Span::default(),
            states: vec!["A".to_string(), "B".to_string()],
            transitions: vec![],
            initial_state: Some("C".to_string()),
        };
        assert!(
            EffectStateMachine::from_effect_def(&def).is_none(),
            "Should reject initial state not in declared states"
        );
    }

    #[test]
    fn test_effect_state_machine_invalid_transition_state() {
        use crate::parser::EffectTransition;
        // transition references undeclared state "X" → should return None
        let def = crate::parser::EffectDef {
            name: "Bad".to_string(),
            params: vec![],
            constraint: None,
            includes: vec![],
            refinement: None,
            parent: vec![],
            span: Span::default(),
            states: vec!["A".to_string(), "B".to_string()],
            transitions: vec![EffectTransition {
                operation: "go".to_string(),
                from_state: "A".to_string(),
                to_state: "X".to_string(), // undeclared
            }],
            initial_state: Some("A".to_string()),
        };
        assert!(
            EffectStateMachine::from_effect_def(&def).is_none(),
            "Should reject transition referencing undeclared state"
        );
    }

    #[test]
    fn test_effect_state_machine_transitions() {
        let def = make_file_effect_def();
        let sm = EffectStateMachine::from_effect_def(&def).unwrap();

        assert!(sm.can_transition("write", "Open"));
        assert!(!sm.can_transition("write", "Closed"));
        assert!(sm.can_transition("open", "Closed"));
        assert!(!sm.can_transition("open", "Open"));

        assert_eq!(sm.next_state("open", "Closed"), Some(&"Open".to_string()));
        assert_eq!(sm.next_state("close", "Open"), Some(&"Closed".to_string()));
        assert_eq!(sm.next_state("write", "Closed"), None);
    }

    #[test]
    fn test_temporal_effect_valid_sequence() {
        // open → write → close (valid sequence)
        let def = make_file_effect_def();
        let sm = EffectStateMachine::from_effect_def(&def).unwrap();
        let mut sms = HashMap::new();
        sms.insert("File".to_string(), sm);

        let body = MirBody {
            name: "test_valid".to_string(),
            locals: vec![LocalDecl {
                local: Local(0),
                name: Some("_ret".to_string()),
                ty: None,
                movability: Movability::Move,
            }],
            blocks: vec![
                BasicBlock {
                    id: 0,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "open".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::Goto(1),
                },
                BasicBlock {
                    id: 1,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "write".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::Goto(2),
                },
                BasicBlock {
                    id: 2,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "close".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::Return(Operand::Constant(
                        crate::mir::MirConstant::Int(0),
                    )),
                },
            ],
            entry_block: 0,
        };

        let result = analyze_temporal_effects(&body, &sms);
        assert!(
            result.violations.is_empty(),
            "Valid sequence should have no violations, got: {:?}",
            result.violations
        );
    }

    #[test]
    fn test_temporal_effect_invalid_prestate() {
        // write before open → InvalidPreState
        let def = make_file_effect_def();
        let sm = EffectStateMachine::from_effect_def(&def).unwrap();
        let mut sms = HashMap::new();
        sms.insert("File".to_string(), sm);

        let body = MirBody {
            name: "test_invalid".to_string(),
            locals: vec![LocalDecl {
                local: Local(0),
                name: Some("_ret".to_string()),
                ty: None,
                movability: Movability::Move,
            }],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![MirStatement::Assign(
                    Place::Local(Local(0)),
                    Rvalue::Perform {
                        effect: "File".to_string(),
                        operation: "write".to_string(),
                        args: vec![],
                    },
                )],
                terminator: Terminator::Return(Operand::Constant(crate::mir::MirConstant::Int(0))),
            }],
            entry_block: 0,
        };

        let result = analyze_temporal_effects(&body, &sms);
        assert_eq!(result.violations.len(), 1);
        assert_eq!(
            result.violations[0].kind,
            TemporalViolationKind::InvalidPreState
        );
        assert_eq!(result.violations[0].effect, "File");
        assert_eq!(result.violations[0].operation, "write");
        assert_eq!(result.violations[0].actual_state, "Closed");
    }

    #[test]
    fn test_temporal_effect_invalid_prestate_no_cascade() {
        // write → close when initial state is Closed.
        // write on Closed → InvalidPreState (correct).
        // After error recovery, state becomes Open (write's normal target).
        // close on Open → valid (no second violation).
        // Without error recovery, close on Closed would produce a second false violation.
        let def = make_file_effect_def();
        let sm = EffectStateMachine::from_effect_def(&def).unwrap();
        let mut sms = HashMap::new();
        sms.insert("File".to_string(), sm);

        let body = MirBody {
            name: "test_no_cascade".to_string(),
            locals: vec![LocalDecl {
                local: Local(0),
                name: Some("_ret".to_string()),
                ty: None,
                movability: Movability::Move,
            }],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "write".to_string(),
                            args: vec![],
                        },
                    ),
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "close".to_string(),
                            args: vec![],
                        },
                    ),
                ],
                terminator: Terminator::Return(Operand::Constant(crate::mir::MirConstant::Int(0))),
            }],
            entry_block: 0,
        };

        let result = analyze_temporal_effects(&body, &sms);
        // Only one violation: write on Closed. close on Open (recovered) is valid.
        assert_eq!(
            result.violations.len(),
            1,
            "Should have exactly 1 violation (no cascade), got: {:?}",
            result.violations
        );
        assert_eq!(
            result.violations[0].kind,
            TemporalViolationKind::InvalidPreState
        );
        assert_eq!(result.violations[0].operation, "write");
    }

    #[test]
    fn test_temporal_effect_conflicting_merge() {
        // Branch: one side closes, other doesn't → ConflictingState
        let def = make_file_effect_def();
        let sm = EffectStateMachine::from_effect_def(&def).unwrap();
        let mut sms = HashMap::new();
        sms.insert("File".to_string(), sm);

        let body = MirBody {
            name: "test_conflict".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("_ret".to_string()),
                    ty: None,
                    movability: Movability::Move,
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("cond".to_string()),
                    ty: None,
                    movability: Movability::Move,
                },
            ],
            blocks: vec![
                // Block 0: open, then branch
                BasicBlock {
                    id: 0,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "open".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::SwitchInt {
                        discr: Operand::Place(Place::Local(Local(1))),
                        targets: vec![(1, 1)],
                        otherwise: 2,
                    },
                },
                // Block 1: close
                BasicBlock {
                    id: 1,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "close".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::Goto(3),
                },
                // Block 2: no close (File still Open)
                BasicBlock {
                    id: 2,
                    statements: vec![],
                    terminator: Terminator::Goto(3),
                },
                // Block 3: merge point
                BasicBlock {
                    id: 3,
                    statements: vec![],
                    terminator: Terminator::Return(Operand::Constant(
                        crate::mir::MirConstant::Int(0),
                    )),
                },
            ],
            entry_block: 0,
        };

        let result = analyze_temporal_effects(&body, &sms);
        let conflict_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.kind == TemporalViolationKind::ConflictingState)
            .collect();
        assert!(
            !conflict_violations.is_empty(),
            "Should detect conflicting state at merge point"
        );
        assert_eq!(conflict_violations[0].block_id, 3);
    }

    #[test]
    fn test_temporal_effect_both_branches_close() {
        // Both branches close → no conflict
        let def = make_file_effect_def();
        let sm = EffectStateMachine::from_effect_def(&def).unwrap();
        let mut sms = HashMap::new();
        sms.insert("File".to_string(), sm);

        let body = MirBody {
            name: "test_both_close".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("_ret".to_string()),
                    ty: None,
                    movability: Movability::Move,
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("cond".to_string()),
                    ty: None,
                    movability: Movability::Move,
                },
            ],
            blocks: vec![
                BasicBlock {
                    id: 0,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "open".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::SwitchInt {
                        discr: Operand::Place(Place::Local(Local(1))),
                        targets: vec![(1, 1)],
                        otherwise: 2,
                    },
                },
                // Block 1: write then close
                BasicBlock {
                    id: 1,
                    statements: vec![
                        MirStatement::Assign(
                            Place::Local(Local(0)),
                            Rvalue::Perform {
                                effect: "File".to_string(),
                                operation: "write".to_string(),
                                args: vec![],
                            },
                        ),
                        MirStatement::Assign(
                            Place::Local(Local(0)),
                            Rvalue::Perform {
                                effect: "File".to_string(),
                                operation: "close".to_string(),
                                args: vec![],
                            },
                        ),
                    ],
                    terminator: Terminator::Goto(3),
                },
                // Block 2: just close
                BasicBlock {
                    id: 2,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "close".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::Goto(3),
                },
                BasicBlock {
                    id: 3,
                    statements: vec![],
                    terminator: Terminator::Return(Operand::Constant(
                        crate::mir::MirConstant::Int(0),
                    )),
                },
            ],
            entry_block: 0,
        };

        let result = analyze_temporal_effects(&body, &sms);
        assert!(
            result.violations.is_empty(),
            "Both branches close should have no violations, got: {:?}",
            result.violations
        );
    }

    #[test]
    fn test_temporal_effect_loop() {
        // Loop: open → (write)* → close (write is Open→Open, loop OK)
        let def = make_file_effect_def();
        let sm = EffectStateMachine::from_effect_def(&def).unwrap();
        let mut sms = HashMap::new();
        sms.insert("File".to_string(), sm);

        let body = MirBody {
            name: "test_loop".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("_ret".to_string()),
                    ty: None,
                    movability: Movability::Move,
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("cond".to_string()),
                    ty: None,
                    movability: Movability::Move,
                },
            ],
            blocks: vec![
                // Block 0: open, goto loop header
                BasicBlock {
                    id: 0,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "open".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::Goto(1),
                },
                // Block 1: loop header — branch to body or exit
                BasicBlock {
                    id: 1,
                    statements: vec![],
                    terminator: Terminator::SwitchInt {
                        discr: Operand::Place(Place::Local(Local(1))),
                        targets: vec![(1, 2)],
                        otherwise: 3,
                    },
                },
                // Block 2: loop body — write, back to header
                BasicBlock {
                    id: 2,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "write".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::Goto(1),
                },
                // Block 3: after loop — close
                BasicBlock {
                    id: 3,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "close".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::Return(Operand::Constant(
                        crate::mir::MirConstant::Int(0),
                    )),
                },
            ],
            entry_block: 0,
        };

        let result = analyze_temporal_effects(&body, &sms);
        assert!(
            result.violations.is_empty(),
            "Loop with Open→Open write should have no violations, got: {:?}",
            result.violations
        );
    }

    // =========================================================================
    // PII Pipeline Tests (Plan 22)
    // =========================================================================

    /// Helper: create a DataPipeline effect with states [Raw, Anonymized].
    fn make_data_pipeline_effect_def() -> crate::parser::EffectDef {
        use crate::parser::EffectTransition;
        crate::parser::EffectDef {
            name: "DataPipeline".to_string(),
            params: vec![],
            constraint: None,
            includes: vec![],
            refinement: None,
            parent: vec![],
            span: Span::default(),
            states: vec!["Raw".to_string(), "Anonymized".to_string()],
            transitions: vec![
                EffectTransition {
                    operation: "load".to_string(),
                    from_state: "Raw".to_string(),
                    to_state: "Raw".to_string(),
                },
                EffectTransition {
                    operation: "anonymize".to_string(),
                    from_state: "Raw".to_string(),
                    to_state: "Anonymized".to_string(),
                },
                EffectTransition {
                    operation: "log".to_string(),
                    from_state: "Anonymized".to_string(),
                    to_state: "Anonymized".to_string(),
                },
            ],
            initial_state: Some("Raw".to_string()),
        }
    }

    #[test]
    fn test_pii_pipeline_valid_sequence() {
        // load → anonymize → log (valid sequence)
        let def = make_data_pipeline_effect_def();
        let sm = EffectStateMachine::from_effect_def(&def).unwrap();
        let mut sms = HashMap::new();
        sms.insert("DataPipeline".to_string(), sm);

        let body = MirBody {
            name: "test_pii_valid".to_string(),
            locals: vec![LocalDecl {
                local: Local(0),
                name: Some("_ret".to_string()),
                ty: None,
                movability: Movability::Move,
            }],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "DataPipeline".to_string(),
                            operation: "load".to_string(),
                            args: vec![],
                        },
                    ),
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "DataPipeline".to_string(),
                            operation: "anonymize".to_string(),
                            args: vec![],
                        },
                    ),
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "DataPipeline".to_string(),
                            operation: "log".to_string(),
                            args: vec![],
                        },
                    ),
                ],
                terminator: Terminator::Return(Operand::Constant(crate::mir::MirConstant::Int(0))),
            }],
            entry_block: 0,
        };

        let result = analyze_temporal_effects(&body, &sms);
        assert!(
            result.violations.is_empty(),
            "Valid PII pipeline (load → anonymize → log) should have no violations, got: {:?}",
            result.violations
        );
    }

    #[test]
    fn test_pii_pipeline_skip_anonymize() {
        // load → log (skipping anonymize) → InvalidPreState
        let def = make_data_pipeline_effect_def();
        let sm = EffectStateMachine::from_effect_def(&def).unwrap();
        let mut sms = HashMap::new();
        sms.insert("DataPipeline".to_string(), sm);

        let body = MirBody {
            name: "test_pii_skip_anonymize".to_string(),
            locals: vec![LocalDecl {
                local: Local(0),
                name: Some("_ret".to_string()),
                ty: None,
                movability: Movability::Move,
            }],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "DataPipeline".to_string(),
                            operation: "load".to_string(),
                            args: vec![],
                        },
                    ),
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "DataPipeline".to_string(),
                            operation: "log".to_string(),
                            args: vec![],
                        },
                    ),
                ],
                terminator: Terminator::Return(Operand::Constant(crate::mir::MirConstant::Int(0))),
            }],
            entry_block: 0,
        };

        let result = analyze_temporal_effects(&body, &sms);
        assert_eq!(
            result.violations.len(),
            1,
            "Should have exactly 1 violation for skipping anonymize, got: {:?}",
            result.violations
        );
        assert_eq!(
            result.violations[0].kind,
            TemporalViolationKind::InvalidPreState
        );
        assert_eq!(result.violations[0].effect, "DataPipeline");
        assert_eq!(result.violations[0].operation, "log");
        assert_eq!(result.violations[0].actual_state, "Raw");
        assert!(
            result.violations[0].expected_state.contains("Anonymized"),
            "Expected state should mention 'Anonymized', got: {}",
            result.violations[0].expected_state
        );
    }

    #[test]
    fn test_pii_pipeline_branch_conflict() {
        // Branch: one side performs anonymize, other does not, then both merge to log
        // → ConflictingState at merge point
        let def = make_data_pipeline_effect_def();
        let sm = EffectStateMachine::from_effect_def(&def).unwrap();
        let mut sms = HashMap::new();
        sms.insert("DataPipeline".to_string(), sm);

        let body = MirBody {
            name: "test_pii_branch_conflict".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("_ret".to_string()),
                    ty: None,
                    movability: Movability::Move,
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("cond".to_string()),
                    ty: None,
                    movability: Movability::Move,
                },
            ],
            blocks: vec![
                // Block 0: load, then branch
                BasicBlock {
                    id: 0,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "DataPipeline".to_string(),
                            operation: "load".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::SwitchInt {
                        discr: Operand::Place(Place::Local(Local(1))),
                        targets: vec![(1, 1)],
                        otherwise: 2,
                    },
                },
                // Block 1: anonymize (DataPipeline goes to Anonymized)
                BasicBlock {
                    id: 1,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "DataPipeline".to_string(),
                            operation: "anonymize".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::Goto(3),
                },
                // Block 2: no anonymize (DataPipeline stays Raw)
                BasicBlock {
                    id: 2,
                    statements: vec![],
                    terminator: Terminator::Goto(3),
                },
                // Block 3: merge point → log
                BasicBlock {
                    id: 3,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "DataPipeline".to_string(),
                            operation: "log".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::Return(Operand::Constant(
                        crate::mir::MirConstant::Int(0),
                    )),
                },
            ],
            entry_block: 0,
        };

        let result = analyze_temporal_effects(&body, &sms);
        let conflict_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.kind == TemporalViolationKind::ConflictingState)
            .collect();
        assert!(
            !conflict_violations.is_empty(),
            "Should detect ConflictingState at merge point (one branch Anonymized, other Raw)"
        );
        assert_eq!(
            conflict_violations[0].block_id, 3,
            "ConflictingState should be at block 3 (merge point)"
        );
        assert_eq!(conflict_violations[0].effect, "DataPipeline");
    }

    // =========================================================================
    // Modular Verification (effect_pre / effect_post) tests — Plan 24
    // =========================================================================

    fn make_file_effect_sm() -> EffectStateMachine {
        let def = make_file_effect_def();
        EffectStateMachine::from_effect_def(&def).unwrap()
    }

    #[test]
    fn test_modular_verification_valid_chain() {
        // Two atoms with matching pre/post contracts called in sequence — no violations.
        // open_file: effect_pre={File:Closed}, effect_post={File:Open}
        //   body: perform File.open
        // We verify open_file body with initial state overridden to Closed.
        let mut sm = make_file_effect_sm();
        // Override initial state per effect_pre: File=Closed (already default)
        sm.initial_state = "Closed".to_string();

        let body = MirBody {
            name: "test_modular_valid".to_string(),
            locals: vec![LocalDecl {
                local: Local(0),
                name: Some("_ret".to_string()),
                ty: None,
                movability: Movability::Move,
            }],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![MirStatement::Assign(
                    Place::Local(Local(0)),
                    Rvalue::Perform {
                        effect: "File".to_string(),
                        operation: "open".to_string(),
                        args: vec![],
                    },
                )],
                terminator: Terminator::Return(Operand::Constant(MirConstant::Int(0))),
            }],
            entry_block: 0,
        };
        let mut machines = HashMap::new();
        machines.insert("File".to_string(), sm);
        let result = analyze_temporal_effects(&body, &machines);
        assert!(
            result.violations.is_empty(),
            "open_file body with effect_pre={{File:Closed}} should have no violations"
        );
        // Check exit state is Open (matching effect_post)
        if let Some(exit_map) = result.exit_states.get(&0) {
            assert_eq!(exit_map.get("File").unwrap(), "Open");
        }
    }

    #[test]
    fn test_modular_verification_pre_mismatch() {
        // Caller's current state doesn't match callee's effect_pre — InvalidPreState.
        // Atom declares effect_pre={File:Open} but body starts with File.open
        // which requires Closed state. We override initial to Open (per effect_pre).
        let mut sm = make_file_effect_sm();
        sm.initial_state = "Open".to_string(); // effect_pre says File must be Open

        let body = MirBody {
            name: "test_modular_pre_mismatch".to_string(),
            locals: vec![LocalDecl {
                local: Local(0),
                name: Some("_ret".to_string()),
                ty: None,
                movability: Movability::Move,
            }],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![
                    // open requires Closed, but we're in Open → InvalidPreState
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "open".to_string(),
                            args: vec![],
                        },
                    ),
                ],
                terminator: Terminator::Return(Operand::Constant(MirConstant::Int(0))),
            }],
            entry_block: 0,
        };
        let mut machines = HashMap::new();
        machines.insert("File".to_string(), sm);
        let result = analyze_temporal_effects(&body, &machines);
        assert!(
            !result.violations.is_empty(),
            "Should detect InvalidPreState when open is called in Open state"
        );
        assert_eq!(
            result.violations[0].kind,
            TemporalViolationKind::InvalidPreState
        );
        assert_eq!(result.violations[0].operation, "open");
    }

    #[test]
    fn test_modular_verification_post_check() {
        // Atom body's final state doesn't match declared effect_post.
        // effect_pre={File:Closed}, effect_post={File:Closed}
        // body: perform File.open (leaves File in Open, not Closed)
        let mut sm = make_file_effect_sm();
        sm.initial_state = "Closed".to_string();

        let body = MirBody {
            name: "test_modular_post_check".to_string(),
            locals: vec![LocalDecl {
                local: Local(0),
                name: Some("_ret".to_string()),
                ty: None,
                movability: Movability::Move,
            }],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "open".to_string(),
                            args: vec![],
                        },
                    ),
                    // File is now Open, but effect_post expects Closed
                ],
                terminator: Terminator::Return(Operand::Constant(MirConstant::Int(0))),
            }],
            entry_block: 0,
        };
        let mut machines = HashMap::new();
        machines.insert("File".to_string(), sm);
        let result = analyze_temporal_effects(&body, &machines);
        assert!(
            result.violations.is_empty(),
            "No inline violations (open from Closed is valid)"
        );
        // Check exit state is Open (not Closed) — verification pipeline would
        // detect mismatch with effect_post={File:Closed}
        if let Some(exit_map) = result.exit_states.get(&0) {
            assert_eq!(
                exit_map.get("File").unwrap(),
                "Open",
                "Exit state should be Open, mismatching effect_post Closed"
            );
        }
    }

    // =========================================================================
    // Cross-atom contract composition tests (P2-A)
    // =========================================================================

    /// Helper: build callee contracts for open_file and write_and_close atoms.
    fn make_file_callee_contracts() -> HashMap<String, AtomEffectContract> {
        let mut contracts = HashMap::new();
        // open_file: effect_pre={File:Closed}, effect_post={File:Open}
        let mut open_pre = HashMap::new();
        open_pre.insert("File".to_string(), "Closed".to_string());
        let mut open_post = HashMap::new();
        open_post.insert("File".to_string(), "Open".to_string());
        contracts.insert(
            "open_file".to_string(),
            AtomEffectContract {
                effect_pre: open_pre,
                effect_post: open_post,
            },
        );
        // write_and_close: effect_pre={File:Open}, effect_post={File:Closed}
        let mut wc_pre = HashMap::new();
        wc_pre.insert("File".to_string(), "Open".to_string());
        let mut wc_post = HashMap::new();
        wc_post.insert("File".to_string(), "Closed".to_string());
        contracts.insert(
            "write_and_close".to_string(),
            AtomEffectContract {
                effect_pre: wc_pre,
                effect_post: wc_post,
            },
        );
        contracts
    }

    fn make_ownership_effect_sm() -> EffectStateMachine {
        use crate::parser::{EffectDef, EffectTransition};
        let def = EffectDef {
            name: "Ownership".to_string(),
            params: vec![],
            constraint: None,
            includes: vec![],
            refinement: None,
            parent: vec![],
            span: Span::default(),
            states: vec![
                "Idle".to_string(),
                "PendingTransfer".to_string(),
                "Transferred".to_string(),
            ],
            transitions: vec![
                EffectTransition {
                    operation: "propose".to_string(),
                    from_state: "Idle".to_string(),
                    to_state: "PendingTransfer".to_string(),
                },
                EffectTransition {
                    operation: "accept".to_string(),
                    from_state: "PendingTransfer".to_string(),
                    to_state: "Transferred".to_string(),
                },
                EffectTransition {
                    operation: "cancel".to_string(),
                    from_state: "PendingTransfer".to_string(),
                    to_state: "Idle".to_string(),
                },
            ],
            initial_state: Some("Idle".to_string()),
        };
        EffectStateMachine::from_effect_def(&def).unwrap()
    }

    fn make_ownership_callee_contracts() -> HashMap<String, AtomEffectContract> {
        let mut contracts = HashMap::new();

        let mut propose_pre = HashMap::new();
        propose_pre.insert("Ownership".to_string(), "Idle".to_string());
        let mut propose_post = HashMap::new();
        propose_post.insert("Ownership".to_string(), "PendingTransfer".to_string());
        contracts.insert(
            "propose_transfer".to_string(),
            AtomEffectContract {
                effect_pre: propose_pre,
                effect_post: propose_post,
            },
        );

        let mut accept_pre = HashMap::new();
        accept_pre.insert("Ownership".to_string(), "PendingTransfer".to_string());
        let mut accept_post = HashMap::new();
        accept_post.insert("Ownership".to_string(), "Transferred".to_string());
        contracts.insert(
            "accept_transfer".to_string(),
            AtomEffectContract {
                effect_pre: accept_pre,
                effect_post: accept_post,
            },
        );

        let mut cancel_pre = HashMap::new();
        cancel_pre.insert("Ownership".to_string(), "PendingTransfer".to_string());
        let mut cancel_post = HashMap::new();
        cancel_post.insert("Ownership".to_string(), "Idle".to_string());
        contracts.insert(
            "cancel_transfer".to_string(),
            AtomEffectContract {
                effect_pre: cancel_pre,
                effect_post: cancel_post,
            },
        );

        contracts
    }

    #[test]
    fn test_cross_atom_composition_valid() {
        // full_pipeline: open_file → write_and_close (correct order)
        // File starts at Closed, open_file requires Closed and produces Open,
        // write_and_close requires Open and produces Closed.
        let sm = make_file_effect_sm();
        let contracts = make_file_callee_contracts();

        let body = MirBody {
            name: "full_pipeline".to_string(),
            locals: vec![LocalDecl {
                local: Local(0),
                name: Some("_ret".to_string()),
                ty: None,
                movability: Movability::Move,
            }],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Call {
                            func: "open_file".to_string(),
                            args: vec![Operand::Constant(MirConstant::Int(1))],
                        },
                    ),
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Call {
                            func: "write_and_close".to_string(),
                            args: vec![Operand::Constant(MirConstant::Int(1))],
                        },
                    ),
                ],
                terminator: Terminator::Return(Operand::Constant(MirConstant::Int(0))),
            }],
            entry_block: 0,
        };
        let mut machines = HashMap::new();
        machines.insert("File".to_string(), sm);
        let result = analyze_temporal_effects_with_contracts(&body, &machines, Some(&contracts));
        assert!(
            result.violations.is_empty(),
            "open_file → write_and_close should have no violations, got: {:?}",
            result.violations
        );
        // Exit state should be Closed (write_and_close's effect_post)
        if let Some(exit_map) = result.exit_states.get(&0) {
            assert_eq!(exit_map.get("File").unwrap(), "Closed");
        }
    }

    #[test]
    fn test_cross_atom_composition_invalid_order() {
        // bad_pipeline: write_and_close → open_file (wrong order)
        // File starts at Closed, write_and_close requires Open → InvalidPreState
        let sm = make_file_effect_sm();
        let contracts = make_file_callee_contracts();

        let body = MirBody {
            name: "bad_pipeline".to_string(),
            locals: vec![LocalDecl {
                local: Local(0),
                name: Some("_ret".to_string()),
                ty: None,
                movability: Movability::Move,
            }],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![
                    // write_and_close requires File:Open but current is Closed
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Call {
                            func: "write_and_close".to_string(),
                            args: vec![Operand::Constant(MirConstant::Int(1))],
                        },
                    ),
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Call {
                            func: "open_file".to_string(),
                            args: vec![Operand::Constant(MirConstant::Int(1))],
                        },
                    ),
                ],
                terminator: Terminator::Return(Operand::Constant(MirConstant::Int(0))),
            }],
            entry_block: 0,
        };
        let mut machines = HashMap::new();
        machines.insert("File".to_string(), sm);
        let result = analyze_temporal_effects_with_contracts(&body, &machines, Some(&contracts));
        assert!(
            !result.violations.is_empty(),
            "write_and_close before open_file should produce InvalidPreState"
        );
        assert_eq!(
            result.violations[0].kind,
            TemporalViolationKind::InvalidPreState
        );
        assert!(result.violations[0].operation.contains("write_and_close"));
    }

    #[test]
    fn test_cross_atom_composition_no_contracts() {
        // Atoms without effect_pre/effect_post should pass through without violations.
        let sm = make_file_effect_sm();
        // No callee contracts — acts like plain calls with no effect tracking
        let contracts: HashMap<String, AtomEffectContract> = HashMap::new();

        let body = MirBody {
            name: "no_contract_pipeline".to_string(),
            locals: vec![LocalDecl {
                local: Local(0),
                name: Some("_ret".to_string()),
                ty: None,
                movability: Movability::Move,
            }],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Call {
                            func: "some_function".to_string(),
                            args: vec![Operand::Constant(MirConstant::Int(1))],
                        },
                    ),
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Call {
                            func: "another_function".to_string(),
                            args: vec![Operand::Constant(MirConstant::Int(2))],
                        },
                    ),
                ],
                terminator: Terminator::Return(Operand::Constant(MirConstant::Int(0))),
            }],
            entry_block: 0,
        };
        let mut machines = HashMap::new();
        machines.insert("File".to_string(), sm);
        let result = analyze_temporal_effects_with_contracts(&body, &machines, Some(&contracts));
        assert!(
            result.violations.is_empty(),
            "Calls without effect contracts should produce no violations"
        );
    }

    #[test]
    fn test_cross_atom_composition_chained_abc() {
        // Chained calls: A → B → C (open_file → read_file → write_and_close)
        // File starts at Closed.
        // open_file: pre=Closed, post=Open
        // read_file: pre=Open, post=Open
        // write_and_close: pre=Open, post=Closed
        // Chain: Closed → Open → Open → Closed (valid)
        let sm = make_file_effect_sm();
        let mut contracts = make_file_callee_contracts();
        // Add read_file: effect_pre={File:Open}, effect_post={File:Open}
        let mut read_pre = HashMap::new();
        read_pre.insert("File".to_string(), "Open".to_string());
        let mut read_post = HashMap::new();
        read_post.insert("File".to_string(), "Open".to_string());
        contracts.insert(
            "read_file".to_string(),
            AtomEffectContract {
                effect_pre: read_pre,
                effect_post: read_post,
            },
        );

        let body = MirBody {
            name: "chained_abc".to_string(),
            locals: vec![LocalDecl {
                local: Local(0),
                name: Some("_ret".to_string()),
                ty: None,
                movability: Movability::Move,
            }],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Call {
                            func: "open_file".to_string(),
                            args: vec![Operand::Constant(MirConstant::Int(1))],
                        },
                    ),
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Call {
                            func: "read_file".to_string(),
                            args: vec![Operand::Constant(MirConstant::Int(1))],
                        },
                    ),
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Call {
                            func: "write_and_close".to_string(),
                            args: vec![Operand::Constant(MirConstant::Int(1))],
                        },
                    ),
                ],
                terminator: Terminator::Return(Operand::Constant(MirConstant::Int(0))),
            }],
            entry_block: 0,
        };
        let mut machines = HashMap::new();
        machines.insert("File".to_string(), sm);
        let result = analyze_temporal_effects_with_contracts(&body, &machines, Some(&contracts));
        assert!(
            result.violations.is_empty(),
            "open_file → read_file → write_and_close should have no violations, got: {:?}",
            result.violations
        );
        // Exit state should be Closed (write_and_close's effect_post)
        if let Some(exit_map) = result.exit_states.get(&0) {
            assert_eq!(exit_map.get("File").unwrap(), "Closed");
        }
    }

    #[test]
    fn test_cross_atom_composition_effect_post_available_to_caller() {
        // After calling open_file (post=Open), a direct perform File.write should succeed
        // because the caller now sees File:Open from the callee's effect_post.
        let sm = make_file_effect_sm();
        let contracts = make_file_callee_contracts();

        let body = MirBody {
            name: "post_available".to_string(),
            locals: vec![LocalDecl {
                local: Local(0),
                name: Some("_ret".to_string()),
                ty: None,
                movability: Movability::Move,
            }],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![
                    // Call open_file: Closed → Open
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Call {
                            func: "open_file".to_string(),
                            args: vec![Operand::Constant(MirConstant::Int(1))],
                        },
                    ),
                    // Direct perform File.write — valid because open_file's effect_post set File to Open
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "write".to_string(),
                            args: vec![],
                        },
                    ),
                ],
                terminator: Terminator::Return(Operand::Constant(MirConstant::Int(0))),
            }],
            entry_block: 0,
        };
        let mut machines = HashMap::new();
        machines.insert("File".to_string(), sm);
        let result = analyze_temporal_effects_with_contracts(&body, &machines, Some(&contracts));
        assert!(
            result.violations.is_empty(),
            "perform File.write after open_file should succeed (effect_post = Open), got: {:?}",
            result.violations
        );
    }

    #[test]
    fn test_ownership_protocol_cross_atom_success_and_failure() {
        let sm = make_ownership_effect_sm();
        let contracts = make_ownership_callee_contracts();

        let valid = MirBody {
            name: "full_transfer".to_string(),
            locals: vec![LocalDecl {
                local: Local(0),
                name: Some("_ret".to_string()),
                ty: None,
                movability: Movability::Move,
            }],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Call {
                            func: "propose_transfer".to_string(),
                            args: vec![Operand::Constant(MirConstant::Int(1))],
                        },
                    ),
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Call {
                            func: "accept_transfer".to_string(),
                            args: vec![Operand::Constant(MirConstant::Int(1))],
                        },
                    ),
                ],
                terminator: Terminator::Return(Operand::Constant(MirConstant::Int(0))),
            }],
            entry_block: 0,
        };
        let mut machines = HashMap::new();
        machines.insert("Ownership".to_string(), sm.clone());
        let result = analyze_temporal_effects_with_contracts(&valid, &machines, Some(&contracts));
        assert!(
            result.violations.is_empty(),
            "propose_transfer → accept_transfer should pass, got: {:?}",
            result.violations
        );
        if let Some(exit_map) = result.exit_states.get(&0) {
            assert_eq!(exit_map.get("Ownership").unwrap(), "Transferred");
        }

        let invalid = MirBody {
            name: "hostile_takeover".to_string(),
            locals: vec![LocalDecl {
                local: Local(0),
                name: Some("_ret".to_string()),
                ty: None,
                movability: Movability::Move,
            }],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![MirStatement::Assign(
                    Place::Local(Local(0)),
                    Rvalue::Call {
                        func: "accept_transfer".to_string(),
                        args: vec![Operand::Constant(MirConstant::Int(1))],
                    },
                )],
                terminator: Terminator::Return(Operand::Constant(MirConstant::Int(0))),
            }],
            entry_block: 0,
        };
        let mut machines = HashMap::new();
        machines.insert("Ownership".to_string(), sm);
        let result = analyze_temporal_effects_with_contracts(&invalid, &machines, Some(&contracts));
        assert!(
            !result.violations.is_empty(),
            "accept_transfer from Idle should produce InvalidPreState"
        );
        assert_eq!(
            result.violations[0].kind,
            TemporalViolationKind::InvalidPreState
        );
        assert!(result.violations[0].operation.contains("accept_transfer"));
        assert_eq!(result.violations[0].actual_state, "Idle");
        assert_eq!(result.violations[0].expected_state, "PendingTransfer");
    }
}
