type Pos = i64 where v > 0;
struct P { x: Pos, y: i64 }
atom gx(p: P) -> i64 requires: true; ensures: result >= 1; body: { p.x };
atom mk() -> i64 requires: true; ensures: result >= 1; body: { let q = P { x: 0, y: 1 }; gx(q) };
