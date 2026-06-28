// =============================================================
// std/algebra/finite_field — finite-field equality helpers
// =============================================================
// Z3-side zero representative used by the Lean finite-field equality bridge fixture.

atom ff_zero_eq_zero(p: i64)
    requires: p > 0;
    ensures: result == 0;
    body: {
        0
    };
