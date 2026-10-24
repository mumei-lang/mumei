// Negative: `mystery<i64>(3)` parses as an explicit-type call but no atom
// (or monomorphizable generic) named `mystery` exists — verification must
// fail closed with "Unknown function: mystery<i64>", not pass vacuously.

atom main() -> i64
    requires: true;
    ensures: result == 3;
    body: mystery<i64>(3);
