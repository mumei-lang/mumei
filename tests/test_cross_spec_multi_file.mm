trusted atom cross_file_caller(x: i64) -> i64 {
    requires: x >= 0;
    ensures: result >= 0;
    body: {
        cross_file_callee(x)
    }
}

trusted atom high_global_result(x: i64) -> i64 {
    requires: true;
    ensures: result >= 10;
    body: {
        10
    }
}
