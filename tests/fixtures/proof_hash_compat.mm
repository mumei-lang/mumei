import "proof_hash_compat_lib.mm";

type Usd = i64 unit USD;

struct Range {
    lo: i64 where v >= 0,
    hi: i64 where v >= 0
}

trusted atom ext_clamp(x: i64) -> i64
    requires: true;
    ensures: result >= 0;
    body: x;

atom leaf(x: Pos) -> i64
    requires: true;
    ensures: result > 0;
    body: x;

atom mid(x: Pos) -> i64
    requires: true;
    ensures: result > 1;
    body: lib_inc(leaf(x));

atom top(x: Pos) -> i64
    requires: true;
    ensures: result > 1;
    body: mid(x);

atom span(r: Range) -> i64
    requires: r.hi >= r.lo;
    ensures: result >= 0;
    body: r.hi - r.lo;

atom price(a: Usd, b: Usd) -> Usd
    requires: a >= 0 && b >= 0;
    ensures: result >= a;
    body: a + b;

atom safe(x: i64) -> i64
    requires: true;
    ensures: result >= 0;
    body: ext_clamp(x);
