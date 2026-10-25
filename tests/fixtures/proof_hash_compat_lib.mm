type Pos = i64 where v > 0;
atom lib_inc(x: i64) -> i64
    requires: x >= 0;
    ensures: result > x;
    body: x + 1;
