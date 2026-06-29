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
