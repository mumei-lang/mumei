// Counterexample case: the cumulative dose is summed without clamping, so the
// contract's daily-maximum guarantee is refuted by a counterexample.
// expected: FAIL

atom accumulate_dose_without_clamp(administered: i64, requested: i64)
    requires: administered >= 0 && administered <= 4000 && requested >= 0 && requested <= 1000;
    ensures: result >= 0 && result <= 4000;
    body: administered + requested;
