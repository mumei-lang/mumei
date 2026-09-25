// =============================================================
// std/string_utils — verified string utilities
// =============================================================
// Z3 の文字列理論が扱う長さ・前方/後方一致・包含・検索位置・部分文字列の性質を公開する、検証側のヘルパー。

atom safe_truncate(s: Str, max_len: i64) -> i64
    requires: max_len >= 0;
    ensures: result >= 0 && result <= max_len;
    body: {
        max_len
    };

atom is_ascii(code: i64) -> i64
    requires: true;
    ensures: ((code >= 0 && code <= 127) && result == 1) || ((code < 0 || code > 127) && result == 0);
    body: {
        if code >= 0 && code <= 127 { 1 } else { 0 }
    };

atom has_prefix(s: Str, p: Str) -> i64
    requires: true;
    ensures: (starts_with(s, p) && result == 1) || (!starts_with(s, p) && result == 0);
    body: {
        if starts_with(s, p) { 1 } else { 0 }
    };

atom has_suffix(s: Str, p: Str) -> i64
    requires: true;
    ensures: (ends_with(s, p) && result == 1) || (!ends_with(s, p) && result == 0);
    body: {
        if ends_with(s, p) { 1 } else { 0 }
    };

atom has_substring(s: Str, p: Str) -> i64
    requires: true;
    ensures: (contains(s, p) && result == 1) || (!contains(s, p) && result == 0);
    body: {
        if contains(s, p) { 1 } else { 0 }
    };

atom is_blank(s: Str) -> i64
    requires: true;
    ensures: (is_empty(s) && result == 1) || (!is_empty(s) && result == 0);
    body: {
        if is_empty(s) { 1 } else { 0 }
    };

atom find(s: Str, p: Str) -> i64
    requires: true;
    ensures: result + 1 >= 0 && result <= len(s);
    body: {
        index_of(s, p)
    };

atom is_safe_tmp_path(path: Str) -> i64
    requires: true;
    ensures:
        (starts_with(path, "/tmp/") && not_contains(path, "..") && result == 1)
        || (!(starts_with(path, "/tmp/") && not_contains(path, "..")) && result == 0);
    body: {
        if starts_with(path, "/tmp/") && not_contains(path, "..") { 1 } else { 0 }
    };

atom prefix_len(s: Str, n: i64) -> Str
    requires: n >= 0 && n <= len(s);
    ensures: len(result) == n;
    body: {
        substr(s, 0, n)
    };
