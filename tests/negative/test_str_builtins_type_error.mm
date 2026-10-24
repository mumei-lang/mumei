atom bad_index_of(x: i64) -> i64
    requires: true;
    ensures: result + 1 >= 0;
    body: index_of(x, "a");
