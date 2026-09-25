atom count_range(lo: i64, hi: i64) -> i64
    requires: true;
    ensures: (lo <= hi && result == hi - lo) || (lo > hi && result == 0);
    body: {
        let acc = 0;
        for i in lo..hi invariant: acc == i - lo { acc = acc + 1 };
        acc
    };

atom sum_range_nonneg(lo: i64, hi: i64) -> i64
    requires: lo >= 0;
    ensures: result >= 0;
    body: {
        let acc = 0;
        for i in lo..hi invariant: acc >= 0 { acc = acc + i };
        acc
    };

atom inc(x: i64) -> i64
    requires: true;
    ensures: result == x + 1;
    body: x + 1;

atom dbl(x: i64) -> i64
    requires: true;
    ensures: result == 2 * x;
    body: 2 * x;

atom pipe_demo(x: i64) -> i64
    requires: true;
    ensures: result == 2 * x + 2;
    body: x |> inc |> dbl;
