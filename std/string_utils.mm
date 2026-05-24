// =============================================================
// std/string_utils — verified string utilities
// =============================================================
// 文字列内容は不透明に保ち、Z3 で扱える長さと ASCII code の性質だけを公開する。

atom safe_truncate(s: Str, max_len: i64)
requires: max_len >= 0;
ensures: result >= 0 && result <= max_len;
body: {
    max_len
};

atom is_ascii(code: i64)
requires: true;
ensures: ((code >= 0 && code <= 127) && result == 1) || ((code < 0 || code > 127) && result == 0);
body: {
    if code >= 0 && code <= 127 { 1 } else { 0 }
};
