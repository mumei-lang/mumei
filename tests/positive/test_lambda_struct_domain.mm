struct Box {
    value: u32
}

atom g(b: Box) -> u32
    requires: true;
    ensures: result == b.value;
    body: b.value;

atom f(b: Box) -> u32
    requires: true;
    ensures: result == b.value;
    body: {
        let h = |x: Box| g(x);
        h(b)
    }
