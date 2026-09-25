atom string_is_empty(s: Str) -> bool
    requires: true;
    ensures: result == is_empty(s);
    body: is_empty(s);

atom string_index_of(s: Str, p: Str) -> i64
    requires: true;
    ensures: result + 1 >= 0 && result <= len(s);
    body: index_of(s, p);

atom string_substr(s: Str, n: i64) -> Str
    requires: n >= 0 && n <= len(s);
    ensures: len(result) == n;
    body: substr(s, 0, n);

atom string_char_at(s: Str, i: i64) -> Str
    requires: i >= 0 && i < len(s);
    ensures: len(result) <= 1;
    body: char_at(s, i);

atom string_contains(s: Str, p: Str) -> bool
    requires: true;
    ensures: result == contains(s, p);
    body: contains(s, p);
