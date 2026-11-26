type Pos = i64 where v > 0;
struct P { x: Pos, y: i64 }
atom zero() -> P requires: true; ensures: result.y == 0; body: { P { x: 0, y: 0 } };
