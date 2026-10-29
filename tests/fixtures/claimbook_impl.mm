struct Point { x: i64 }

impl Point {
    atom get(self: Point) -> i64
        requires: true;
        ensures: result == self.x;
        body: self.x;
}
