// `a[i][j]` cannot be expressed — `a[i]` selects an element and the
// trailing `[j]` has no `ArrayAccess` target (the array position only
// accepts a bare identifier). Previously the leftover `[j]` re-lexed as
// a stray array-literal statement, silently verifying a different
// program. Now the parser records a syntax failure and the module is
// rejected before verification.
atom nested_index(a: [i64]) -> i64
requires: len(a) >= 1;
ensures: result >= 0;
body: {
    a[0][0]
};
