atom f(s: Str, len_s: i64) -> bool
    requires: true;
    ensures: result == true;
    body: len_s == len(s);
