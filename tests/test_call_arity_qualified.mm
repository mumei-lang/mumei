// Module-qualified call to a std atom with too few args reports arity.
// Mirrors the latent bug in tests/test_settlement.mm:57.

import "std/settlement" as settlement;

atom check_all(arr: [i64], n: i64)
    requires: n >= 0;
    ensures: true;
    body: settlement::verify_all_balances_non_negative(n);
