type Meter = f64 unit Meter;
type Second = f64 unit Second;

atom position_before_time(pos: Meter, t: Second)
    requires: true;
    ensures: true;
    body: pos < t;
