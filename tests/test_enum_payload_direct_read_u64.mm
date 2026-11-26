struct UPoint {
    x: u64
}

enum UValue {
    Point(p: UPoint),
    Scalar(port: u64, keep: u64),
    Empty
}

atom struct_payload_u64_read_is_unsigned(e: UValue, x: u64) -> bool
requires: match e {
    UValue::Point(p) => p.x == x,
    _ => false
} && x == 0 - 1;
ensures: result == false;
body: e.p.x < 0;

atom scalar_payload_u64_read_is_unsigned(e: UValue, x: u64) -> bool
requires: match e {
    UValue::Scalar(port, _) => port == x,
    _ => false
} && x == 0 - 1;
ensures: result == false;
body: e.port < 0;
