//! Step 6 of the HIR emitter migration: the HIR-owned `HirBinOp`/`HirJoin`/
//! `HirPattern` types must convert to and from their `parser` counterparts
//! losslessly for every variant.

use mumei_core::hir::{HirBinOp, HirJoin, HirPattern};
use mumei_core::parser::{JoinSemantics, Op, Pattern};

#[test]
fn hir_bin_op_round_trip_covers_every_variant() {
    let ops = [
        Op::Add,
        Op::Sub,
        Op::Mul,
        Op::Pow,
        Op::Div,
        Op::Eq,
        Op::Neq,
        Op::Gt,
        Op::Lt,
        Op::Ge,
        Op::Le,
        Op::And,
        Op::Or,
        Op::Implies,
        Op::BitAnd,
        Op::BitOr,
        Op::BitXor,
        Op::Shl,
        Op::Shr,
    ];
    for op in ops {
        let hir: HirBinOp = op.clone().into();
        let back: Op = hir.into();
        assert_eq!(op, back, "round-trip failed for {op:?}");
    }
}

#[test]
fn hir_join_round_trip_covers_every_variant_and_semantics() {
    for join in [JoinSemantics::All, JoinSemantics::Any] {
        let completes = join.completes_after_first_child();
        let cancels = join.cancels_remaining_children();
        let hir: HirJoin = join.clone().into();
        assert_eq!(hir.completes_after_first_child(), completes);
        assert_eq!(hir.cancels_remaining_children(), cancels);
        let back: JoinSemantics = hir.into();
        assert_eq!(format!("{join:?}"), format!("{back:?}"));
    }
}

#[test]
fn hir_pattern_round_trip_covers_every_variant() {
    let patterns = [
        Pattern::Wildcard,
        Pattern::Literal(42),
        Pattern::Literal(-7),
        Pattern::Variable("x".to_string()),
        Pattern::Variant {
            variant_name: "Some".to_string(),
            fields: vec![Pattern::Variable("inner".to_string())],
        },
        Pattern::Variant {
            variant_name: "Cons".to_string(),
            fields: vec![
                Pattern::Literal(1),
                Pattern::Variant {
                    variant_name: "Cons".to_string(),
                    fields: vec![Pattern::Variable("rest".to_string()), Pattern::Wildcard],
                },
            ],
        },
        Pattern::Variant {
            variant_name: "None".to_string(),
            fields: vec![],
        },
    ];
    for pattern in patterns {
        let hir: HirPattern = pattern.clone().into();
        let back: Pattern = hir.into();
        assert_eq!(
            format!("{pattern:?}"),
            format!("{back:?}"),
            "round-trip failed for {pattern:?}"
        );
    }
}
