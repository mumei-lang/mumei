type Pos = i64 where v > 0;
struct P { x: Pos, y: i64 }
atom gx(p: P) -> i64 requires: true; ensures: result >= 2; body: { p.x };
atom fwd(q: P) -> i64 requires: true; ensures: result >= 1; body: { gx(q) };
atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };
