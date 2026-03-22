// =============================================================
// P2-C: impl block parsing tests
// =============================================================
// Tests that `impl StructName { atom ... }` syntax is correctly
// parsed and methods are registered in ModuleEnv.

struct Point { x: i64, y: i64 }

impl Point {
    atom origin() -> Point
        requires: true;
        ensures: result.x == 0 && result.y == 0;
        body: Point { x: 0, y: 0 };

    atom translate(self: Point, dx: i64, dy: i64) -> Point
        requires: true;
        ensures: result.x == self.x + dx && result.y == self.y + dy;
        body: Point { x: self.x + dx, y: self.y + dy };

    atom manhattan_distance(self: Point) -> i64
        requires: self.x >= 0 && self.y >= 0;
        ensures: result == self.x + self.y;
        body: self.x + self.y;
}

struct Counter { value: i64, max: i64 }

impl Counter {
    atom new(max: i64) -> Counter
        requires: max > 0;
        ensures: result.value == 0 && result.max == max;
        body: Counter { value: 0, max: max };

    atom increment(self: Counter) -> Counter
        requires: self.value < self.max;
        ensures: result.value == self.value + 1;
        body: Counter { value: self.value + 1, max: self.max };

    atom get(self: Counter) -> i64
        requires: true;
        ensures: result == self.value;
        body: self.value;
}

// Test using impl block methods from a top-level atom
atom test_point_origin() -> i64
    requires: true;
    ensures: result == 0;
    body: {
        let p = Point::origin();
        Point::manhattan_distance(p)
    }

atom test_counter_increment() -> i64
    requires: true;
    ensures: result == 1;
    body: {
        let c = Counter::new(10);
        let c2 = Counter::increment(c);
        Counter::get(c2)
    }
