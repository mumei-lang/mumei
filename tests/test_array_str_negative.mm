// Negative `[Str]` coverage — every atom must be rejected.

// Wrong claim: requires pins `a[0] == "x"` but ensures asserts `"y"`.
atom str_wrong_claim(a: [Str]) -> i64
requires: len(a) >= 1 && a[0] == "x";
ensures: a[0] == "y";
body: {
    0
};

// Store of a non-string into a `[Str]` is a type error.
atom str_bad_store(a: [Str]) -> i64
requires: len(a) >= 1;
ensures: result == 1;
body: {
    a[0] = 5;
    1
};

// A literal mixing string and non-string elements is a type error.
atom str_mixed_lit() -> i64
ensures: result == 1;
body: {
    let a = ["x", 1];
    1
};

// The body's `["a"]` does not satisfy `result[0] == "b"`.
atom str_bad_return() -> [Str]
ensures: result[0] == "b";
body: {
    ["a"]
};
