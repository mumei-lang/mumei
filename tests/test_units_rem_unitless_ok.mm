type Second = i64 unit Second;
type Byte = i64 unit Byte;

atom slots_before_size(count: i64, period: Second, size: Byte)
    requires: period > 0;
    ensures: true;
    body: count % period < size;
