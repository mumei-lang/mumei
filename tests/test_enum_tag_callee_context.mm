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
