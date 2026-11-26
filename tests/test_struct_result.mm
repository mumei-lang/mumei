type Pos = i64 where v > 0;
struct P { x: Pos, y: i64 }
atom make(a: Pos) -> P requires: true; ensures: result.x == a; body: { P { x: a, y: 2 } };
atom one() -> P requires: true; ensures: result.x == 1; body: { P { x: 1, y: 0 } };
atom usex(a: Pos) -> i64 requires: true; ensures: result == a; body: { let q = make(a); q.x };
