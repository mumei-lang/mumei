type Second = i64 unit Second;
type Byte = i64 unit Byte;

atom remainder_before_size(t: Second, period: Second, size: Byte)
    requires: period > 0;
    ensures: true;
    body: t % period < size;
