// A-1 relational verification: prove two verified atoms agree by
// calling one of them inside `ensures`.
//
// Each call in a spec introduces a fresh symbol `call_<atom>_<n>` that is
// constrained only by the callee's verified `ensures`. The proof therefore
// goes through because calc_v1's ensures pins its result to `2 * x + 1`,
// which Z3 can equate with calc_v2's body.

atom calc_v1(x: i64)
    requires: true;
    ensures: result == 2 * x + 1;
    body: 2 * x + 1;

atom calc_v2(x: i64)
    requires: true;
    ensures: result == x + x + 1;
    body: x + x + 1;

atom v2_matches_v1(x: i64)
    requires: true;
    ensures: result == calc_v1(x);
    body: calc_v2(x);

atom v2_matches_v1_via_let(x: i64)
    requires: true;
    ensures: result == calc_v1(x);
    body: {
        let r = calc_v2(x);
        r
    }
