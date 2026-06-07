trusted atom cross_file_callee(x: i64) -> i64 {
    requires: x >= 5;
    ensures: result >= 0;
    body: {
        x
    }
}

trusted atom low_global_result(x: i64) -> i64 {
    requires: true;
    ensures: result < 0;
    body: {
        -1
    }
}
