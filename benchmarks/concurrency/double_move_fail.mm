// Counterexample case: the same buffer is handed to two owners, which is the
// verification analogue of a double free.
// expected: FAIL

atom move_twice(buf: [i64])
requires: len(buf) >= 1;
ensures: result >= 0;
body: {
    let first_owner = buf;
    let second_owner = buf;
    len(first_owner)
};
