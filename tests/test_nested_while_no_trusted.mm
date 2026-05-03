// Regression test for Task 1A: nested while loops with `i = i + 1` on a
// Copy-typed (i64) induction variable should NOT trigger spurious
// MIR move-analysis errors. Prior to the fix, the inner loop's
// `i = i + 1` was treated as a move of `i` because the surface-syntax
// numeric literal lost its `i64` type during HIR → MIR lowering, leaving
// the local with the default `Movability::Move` classification. Now that
// numeric literal types propagate, both loops verify without `trusted`.

atom nested_while_increments(n: i64)
requires: n >= 0;
ensures: result == 0;
body:
    let i = 0;
    while i < n {
        let j = 0;
        while j < n {
            j = j + 1;
        }
        i = i + 1;
    }
    0;

atom nested_while_with_array_store(n: i64)
requires: n >= 0;
ensures: result == 0;
body:
    let i = 0;
    while i < n {
        let j = 0;
        while j < n {
            arr[j] = 0;
            j = j + 1;
        }
        i = i + 1;
    }
    0;

// Regression for Task 1-A: `let key = arr[i]` inside a nested while loop
// must yield a Copy-typed (`i64`) local. Before the
// `mir.rs::infer_hir_ty()` fix for `Expr::ArrayAccess`, this binding was
// classified `Movability::Move` and tripped a false-positive
// UseAfterMove on the inner loop's `i = i + 1` step.
//
// `forall(i, 0, n, arr[i] >= 0)` in `requires` keeps the OOB checker
// happy by giving Z3 `len_arr >= n + 1` (same idiom as
// `verify_insertion_sort_skeleton` in `tests/test_verified_sort.mm`).
atom nested_while_with_array_read_init(n: i64)
requires: n >= 0 && forall(i, 0, n, arr[i] >= 0);
ensures: result == 0;
body:
    let i = 0;
    while i < n {
        let key = arr[i];
        let j = 0;
        while j < n {
            j = j + 1;
        }
        i = i + 1;
    }
    0;
