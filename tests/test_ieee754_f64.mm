// =============================================================
// IEEE 754 f64 differential fixture (opt-in --ieee754-f64)
// =============================================================
// The ensures `0.1 + 0.2 != 0.3` is TRUE under IEEE 754 binary64
// (rounding: 0.1 + 0.2 rounds to 0.30000000000000004), but FALSE under
// the default exact-rational `Real` encoding (0.1 + 0.2 == 0.3 as ℚ).
//
// Therefore this atom VERIFIES only with `--ieee754-f64` and FAILS in the
// default `Real` mode. It is the differential probe for the opt-in path and
// is intentionally not part of the default-passing fixture suite.
atom ieee754_rounding_holds() -> i64
    requires: true;
    ensures: 0.1 + 0.2 != 0.3;
    body: 0;
