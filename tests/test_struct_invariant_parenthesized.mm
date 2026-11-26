// Regression: parentheses inside a struct invariant are semantic grouping,
// not a flat comparison chain. `(x == y) == true` holds for x = y = false;
// re-normalizing it as `x == y == true` would incorrectly require y = true.
struct FlagPair {
    x: bool,
    y: bool,
    invariant: (self.x == self.y) == true
}

atom false_pair() -> FlagPair
requires: true;
ensures: result.x == false && result.y == false;
body: FlagPair { x: false, y: false };
