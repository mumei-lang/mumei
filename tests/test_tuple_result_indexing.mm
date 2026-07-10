trusted atom SafeAdd(x: u64, y: u64) -> (u64, bool)
requires: x + y <= 2**64 - 1;
ensures: result[0] == x + y && result[1] == false;
body: x + y;
