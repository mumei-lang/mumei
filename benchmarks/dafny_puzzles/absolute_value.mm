// Dafny AbsoluteValue puzzle
atom absolute_value(x: i64)
requires: true;
ensures: result >= 0 && (result == x || result == -x);
body: { if x >= 0 { x } else { -x } };
