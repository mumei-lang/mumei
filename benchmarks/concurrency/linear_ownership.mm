// Linear ownership: a moved buffer is only accessed through its new owner.
// expected: PASS

atom take_buffer(consume buf: [i64])
requires: len(buf) >= 0;
ensures: result >= 0;
body: len(buf);

atom move_once(buf: [i64])
requires: len(buf) >= 1;
ensures: result >= 0;
body: {
    let owned = buf;
    len(owned)
};

atom read_before_move(buf: [i64])
requires: len(buf) >= 1;
ensures: result >= 1;
body: {
    let n = len(buf);
    let owned = buf;
    n
};

atom borrow_without_move(ref buf: [i64])
requires: len(buf) >= 1;
ensures: result >= 1;
body: len(buf);
