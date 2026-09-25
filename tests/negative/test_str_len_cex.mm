atom false_empty(s: Str) -> bool
    requires: true;
    ensures: result == true;
    body: is_empty(s);
