// std/iter.mm
// Collection iteration common interface module

atom count_range(lo: i64, hi: i64) -> i64
    requires: true;
    ensures: (lo <= hi && result == hi - lo) || (lo > hi && result == 0);
    body: {
        let acc = 0;
        for i in lo..hi invariant: acc == i - lo {
            acc = acc + 1
        };
        acc
    };

atom sum_range(lo: i64, hi: i64) -> i64
    requires: lo >= 0;
    ensures: result >= 0;
    body: {
        let acc = 0;
        for i in lo..hi invariant: acc >= 0 {
            acc = acc + i
        };
        acc
    };

atom fold_range(
    lo: i64,
    hi: i64,
    init: i64,
    f: atom_ref(i64, i64) -> i64
) -> i64
    requires: init >= 0;
    ensures: result >= 0;
    contract(f): ensures: result >= 0;
    body: {
        let acc = init;
        for i in lo..hi invariant: acc >= 0 {
            acc = call(f, acc, i)
        };
        acc
    };

atom map_range(lo: i64, hi: i64, f: atom_ref(i64) -> i64) -> i64
    requires: true;
    ensures: (lo <= hi && result == hi - lo) || (lo > hi && result == 0);
    contract(f): ensures: result >= 0;
    body: {
        let n = 0;
        for i in lo..hi invariant: n == i - lo {
            let y = call(f, i);
            n = n + 1
        };
        n
    };

atom sum_map_range(lo: i64, hi: i64, f: atom_ref(i64) -> i64) -> i64
    requires: true;
    ensures: result >= 0;
    contract(f): ensures: result >= 0;
    body: {
        let acc = 0;
        for i in lo..hi invariant: acc >= 0 {
            acc = acc + call(f, i)
        };
        acc
    };
