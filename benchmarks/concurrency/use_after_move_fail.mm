// Counterexample case: the buffer is moved into `owned` and then read again
// through the original binding. MIR move analysis must report use-after-move.
// expected: FAIL

atom read_after_move(buf: [i64])
requires: len(buf) >= 1;
ensures: result >= 0;
body: {
    let owned = buf;
    let n = len(buf);
    n
};
