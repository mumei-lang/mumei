struct UPoint {
    x: u64
}

enum UValue {
    Point(p: UPoint),
    Scalar(port: u64, keep: u64),
    Empty
}

atom bad_struct_payload_signed_compare(e: UValue, x: u64) -> bool
requires: match e {
    UValue::Point(p) => p.x == x,
    _ => false
} && x == 0 - 1;
ensures: result == true;
body: e.p.x < 0;

atom bad_scalar_payload_signed_compare(e: UValue, x: u64) -> bool
requires: match e {
    UValue::Scalar(port, _) => port == x,
    _ => false
} && x == 0 - 1;
ensures: result == true;
body: e.port < 0;
