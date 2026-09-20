// `[Str]` — string array elements encode on Z3's `Seq`/String sort.
// Previously the element sort fell through to `Int`, so `a[i] == "x"`
// could not verify and `a[i] = "s"` failed coercion.

atom str_param_read(a: [Str]) -> i64
requires: len(a) >= 1 && a[0] == "x";
ensures: result == 1;
body: {
    if a[0] == "x" { 1 } else { 0 }
};

atom str_lit_read() -> i64
ensures: result == 1;
body: {
    let a = ["x", "y"];
    if a[1] == "y" && a[0] != "z" { 1 } else { 0 }
};

atom str_param_store(a: [Str]) -> i64
requires: len(a) >= 1;
ensures: result == 1;
body: {
    a[0] = "z";
    if a[0] == "z" { 1 } else { 0 }
};

// The post-state store is visible from ensures.
atom str_post_store(a: [Str]) -> i64
requires: len(a) >= 1;
ensures: a[0] == "z";
body: {
    a[0] = "z";
    1
};

atom str_local_store() -> i64
ensures: result == 1;
body: {
    let a = ["x", "y"];
    a[0] = "w";
    if a[0] == "w" && a[1] == "y" { 1 } else { 0 }
};

atom str_len(a: [Str]) -> i64
requires: len(a) == 2;
ensures: result == 2;
body: {
    len(a)
};

atom str_forall(a: [Str]) -> i64
requires: len(a) == 2 && forall(i, 0, len(a), a[i] == "q");
ensures: result == 1;
body: {
    if a[1] == "q" { 1 } else { 0 }
};

// `let b = a` aliases the same `Int -> Seq` chain.
atom str_alias(a: [Str]) -> i64
requires: len(a) >= 1 && a[0] == "x";
ensures: result == 1;
body: {
    let b = a;
    if b[0] == "x" { 1 } else { 0 }
};

// Elements from two `[Str]` params compare through the string theory.
atom str_param_eq(a: [Str], b: [Str]) -> i64
requires: len(a) >= 1 && len(b) >= 1 && a[0] == b[0];
ensures: result == 1;
body: {
    if a[0] == b[0] { 1 } else { 0 }
};

atom head_str(a: [Str]) -> i64
requires: len(a) >= 1 && a[0] == "p";
ensures: result == 1;
body: {
    if a[0] == "p" { 1 } else { 0 }
};

// A `[Str]` literal call argument satisfies the callee's element contract.
atom str_call_arg() -> i64
ensures: result == 1;
body: {
    head_str(["p", "q"])
};

// `if`-branch arrays merge via `ite` on `Array(Int, Seq)`; `len_<a>`
// picks the branch length and element reads stay `Seq`-sorted.
atom str_if_merge(c: bool) -> i64
requires: true;
ensures: result == 1;
body: {
    let a = if c { ["x", "y"] } else { ["z", "w"] };
    if len(a) == 2 && a[0] != "" { 1 } else { 0 }
};

// A `["s", "t"]` tail wires `__z3_arr_result`/`len_result` so ensures
// can index the returned string array.
atom str_ret_tail() -> [Str]
ensures: len(result) == 2 && result[0] == "s" && result[1] == "t";
body: {
    ["s", "t"]
};

// A while-loop store havocs `a`, but the havoc'd array keeps its
// `Int -> Seq` range — `a[0]` stays a string and `x == "q" || x != "q"`
// (excluded middle on the Seq sort) still proves.
atom str_while_havoc(a: [Str], n: i64) -> i64
requires: len(a) >= 1 && n >= 1;
ensures: result == 1;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        a[0] = "q";
        i = i + 1
    };
    if a[0] == "q" || a[0] != "q" { 1 } else { 0 }
};

// `len` on a bound `[Str]` element resolves through `str.len`.
atom str_elem_len(a: [Str]) -> i64
requires: len(a) >= 1 && a[0] == "abc";
ensures: result == 3;
body: {
    let x = a[0];
    len(x)
};
