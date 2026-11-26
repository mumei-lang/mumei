// No declared `Wrap<...>` use registers an instance, so `Wrap::W` has no
// concrete owner — the generic-qualifier fallback must fail closed, not
// index an empty instance list (regression for a panic).
enum Wrap<T> {
    W(v: T),
    Empty
}
atom pick(e: i64) -> i64
    requires: true;
    ensures: result == 1;
    body: {
        match e {
            Wrap::W => 1,
            _ => 0
        }
    }
