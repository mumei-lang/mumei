atom string_substr_len(s: Str, i: i64, n: i64) -> Str
    requires: i >= 0 && n >= 0 && i + n <= len(s);
    ensures: len(result) == n;
    body: substr(s, i, n);
