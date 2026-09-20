// `[[T]]` — nested arrays have no scalar element sort. Every atom below
// must be rejected: previously `a: [[i64]]` encoded `a[i]` as a phantom
// `Int`, letting `requires: a[0] == 7` wrong-verify `ensures: result == 7`
// on a body returning the (array-typed) `a[0]` — a silent mis-encoding.

// The signature rejection fires before any clause is lowered.
atom nested_param(a: [[i64]]) -> i64
requires: len(a) >= 1 && a[0] == 7;
ensures: result == 7;
body: {
    a[0]
};

atom nested_return() -> [[i64]]
ensures: true;
body: {
    [[1]]
};

// A nested array literal has `Array` elements — no literal range sort.
atom nested_lit() -> i64
ensures: result == 0;
body: {
    let m = [[1, 2], [3]];
    0
};
