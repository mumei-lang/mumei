// Explicit type-argument call syntax: `name<T>(args)` resolves to the
// monomorphized atom `name<T>` — `id<i64>(3)` instantiates `id<T>` and its
// `ensures: result == x` contract is what proves `result == 3` in main.

atom id<T>(x: T) -> T
    requires: true;
    ensures: result == x;
    body: x;

atom main() -> i64
    requires: true;
    ensures: result == 3;
    body: id<i64>(3);
