// =============================================================
// std/algebra/finite_field — finite-field equality helpers
// =============================================================
// Z3 unknown-class finite-field equality witness used by the Lean bridge.

atom ff_zero_eq_zero(p: i64)
    requires: p > 0;
    ensures: ff_eq(result, 0, p);
    body: {
        ff_zero(p)
    };

// Distributivity of multiplication over addition modulo `p`. Z3 returns
// `unknown` (nonlinear + modular), and no bridge lemma template covers the
// iterated `% p` reductions, so the obligation is discharged by the automatic
// tactic search (`mumei-lean` docs/LEAN_TRANSLATOR_SPEC.md §12).
atom ff_mul_add_distributive(a: i64, b: i64, c: i64, p: i64)
    requires: p > 0;
    ensures: ff_eq(result, ff_add(ff_mul(a, b, p), ff_mul(a, c, p), p), p);
    body: {
        ff_mul(a, ff_add(b, c, p), p)
    };
