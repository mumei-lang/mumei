struct LoopNestedInner {
    x: i64
}

struct LoopNestedP {
    inner: LoopNestedInner
}

atom loop_nested_alias_false_claim() -> i64
requires: true;
ensures: result == 0;
body: {
    let q = LoopNestedP { inner: LoopNestedInner { x: 7 } };
    let i = 0;
    while i < 1
    invariant: q.inner.x == 7 && i >= 0 && i <= 1
    decreases: 1 - i
    {
        q = LoopNestedP { inner: LoopNestedInner { x: 7 } };
        i = i + 1;
    };
    let inner = q.inner;
    inner.x
};
