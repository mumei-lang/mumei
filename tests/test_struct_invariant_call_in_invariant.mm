// Invariant expressions may contain commas (call arguments); the struct body
// splitter must not cut them into bogus fields.
atom within(lo: i64, hi: i64) -> bool
requires: true;
ensures: result == (lo <= hi);
body: lo <= hi;

struct Range {
    lo: i64,
    hi: i64,
    invariant: within(self.lo, self.hi),
    width: i64 where v >= 0
}

atom range_new(lo: i64, hi: i64) -> Range
requires: lo <= hi;
ensures: result.lo == lo && result.hi == hi;
body: Range { lo: lo, hi: hi, width: hi - lo };
