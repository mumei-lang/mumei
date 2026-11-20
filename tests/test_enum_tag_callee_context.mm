enum Single {
    Only
}
enum Two {
    A,
    B
}
atom callee_tag(e: Two) -> i64
    requires: true;
    ensures: result == e.tag;
    body: {
        e.tag
    }
atom caller(e: Single, t: Two) -> i64
    requires: true;
    ensures: result == 0;
    body: {
        callee_tag(t)
    }
atom main() -> i64
    requires: true;
    ensures: true;
    body: {
        0
    }
enum Ev { A, B }
struct Holder {
    v: Ev
}
atom callee_struct_tag(h: Holder) -> i64
    requires: true;
    ensures: result == h.v.tag;
    body: {
        h.v.tag
    }
atom caller_struct(h: Ev, s: Holder) -> i64
    requires: s.v.tag == 1;
    ensures: result == 1;
    body: {
        callee_struct_tag(s)
    }
