// =============================================================
// std/string/validator — verified string validators
// =============================================================
// 文字列本体ではなく ASCII コード単位の検証済み述語を提供する。

// ASCII コードが '0'..'9' の範囲なら 1、それ以外なら 0。
atom is_numeric_ascii_code(code: i64)
requires: code >= 0 && code <= 127;
ensures: result == 0 || result == 1;
body: {
    if code >= 48 && code <= 57 { 1 } else { 0 }
};

// ASCII コードが数字または英字なら 1、それ以外なら 0。
atom is_alphanumeric_ascii_code(code: i64)
requires: code >= 0 && code <= 127;
ensures: result == 0 || result == 1;
body: {
    if (code >= 48 && code <= 57) || (code >= 65 && code <= 90) || (code >= 97 && code <= 122) { 1 } else { 0 }
};
