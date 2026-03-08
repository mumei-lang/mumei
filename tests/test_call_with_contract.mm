// =============================================================
// Phase B: call_with_contract verification tests
// =============================================================
// Tests that higher-order functions using call(f, args) with
// contract(f) clauses are fully verified by Z3 without trusted.

// --- Helper atoms ---
atom increment(x: i64)
    requires: x >= 0;
    ensures: result == x + 1;
    body: x + 1;

atom double(x: i64)
    requires: x >= 0;
    ensures: result == x * 2;
    body: x * 2;

atom add(a: i64, b: i64)
    requires: a >= 0 && b >= 0;
    ensures: result == a + b;
    body: a + b;

// --- Test 1: Basic call_with_contract (single arg) ---
// apply calls f via call(f, x) with contract ensuring result >= 0
atom apply(x: i64, f: atom_ref(i64) -> i64)
    requires: x >= 0;
    ensures: result >= 0;
    contract(f): ensures: result >= 0;
    body: call(f, x);

// --- Test 2: call_with_contract with requires and ensures ---
// apply_twice calls f twice; second call needs first result >= 0
atom apply_twice(x: i64, f: atom_ref(i64) -> i64)
    requires: x >= 0;
    ensures: result >= 0;
    contract(f): requires: x >= 0, ensures: result >= 0;
    body: {
        let first = call(f, x);
        call(f, first)
    }

// --- Test 3: Binary function contract ---
atom fold_two(a: i64, b: i64, f: atom_ref(i64, i64) -> i64)
    requires: a >= 0 && b >= 0;
    ensures: result >= 0;
    contract(f): ensures: result >= 0;
    body: call(f, a, b);

// --- Test 4: Callers using concrete atom_ref ---
atom test_apply_increment()
    requires: true;
    ensures: result >= 0;
    body: apply(5, atom_ref(increment));

atom test_apply_twice_double()
    requires: true;
    ensures: result >= 0;
    body: apply_twice(3, atom_ref(double));

atom test_fold_two_add()
    requires: true;
    ensures: result >= 0;
    body: fold_two(3, 4, atom_ref(add));

// --- Test 5: Match with call_with_contract (option map pattern) ---
atom option_map_pattern(opt: i64, f: atom_ref(i64) -> i64)
    requires: opt >= 0 && opt <= 1;
    ensures: result >= 0;
    contract(f): ensures: result >= 0;
    body: {
        match opt {
            0 => 0,
            _ => call(f, opt)
        }
    }
