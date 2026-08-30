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

// Expansion of a modular square into repeated modular multiplication. Z3
// returns `unknown` (nonlinear + modular), and the modular-normalisation stage
// `mumei_ff_mod` does not reach under the exponent, so the obligation is
// discharged by the `mumei_ff_pow` stage of the automatic tactic search
// (`mumei-lean` docs/LEAN_TRANSLATOR_SPEC.md §12).
atom ff_pow_square_expands(a: i64, p: i64)
    requires: p > 0;
    ensures: ff_eq(result, ff_mul(a, a, p), p);
    body: {
        ff_pow(a, 2, p)
    };
