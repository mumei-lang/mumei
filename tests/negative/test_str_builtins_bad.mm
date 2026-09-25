atom bad_is_empty(s: Str) -> bool
    requires: true;
    ensures: is_empty(s);
    body: true;

atom bad_index_of(x: i64) -> i64
    requires: true;
    ensures: result >= -1;
    body: index_of(x, "a");
