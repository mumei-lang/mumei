// Dafny Max puzzle
atom max(a: i64, b: i64)
requires: true;
ensures: result >= a && result >= b && (result == a || result == b);
body: { if a >= b { a } else { b } };
