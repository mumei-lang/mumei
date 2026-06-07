// SV-COMP style integer overflow verification
atom safe_add(a: i64, b: i64)
requires: a >= 0 && b >= 0 && a + b <= 1000000;
ensures: result == a + b && result >= 0;
body: a + b;
