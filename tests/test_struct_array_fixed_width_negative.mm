struct I32Value {
    value: i32
}

struct U32Value {
    value: u32
}

atom i32_array_wrap(values: [I32Value]) -> i64
requires: len(values) > 0;
ensures: result == 1;
body: if values[0].value + 1 > values[0].value { 1 } else { 0 };

atom u32_array_wrap(values: [U32Value]) -> i64
requires: len(values) > 0;
ensures: result == 1;
body: if values[0].value + 1 > values[0].value { 1 } else { 0 };
