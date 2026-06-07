// =============================================================
// std/string/validator — verified string validators
// =============================================================
// 文字列に関連する検証のためのユーティリティ関数群。
// ここでは、ASCII コードが数値または英数字かどうかを判定する
// 基本的な関数を提供する。
// =============================================================

import "std/core" as core;
import "std/string_utils" as string_utils;

// --- is_numeric_ascii_code ---
// ASCII コードが数値（'0'〜'9'）の範囲に属するかどうかを判定する。
// true の場合は 1 を返し、それ以外の場合は 0 を返す。
atom is_numeric_ascii_code(code: i64) -> i64
    requires: code >= 0 && code <= 127;
    ensures: result == 0 || result == 1;
    body: {
        if code >= 48 && code <= 57 { 1 } else { 0 }
    };

// --- is_alphanumeric_ascii_code ---
// ASCII コードが数値または英字（大文字・小文字）の範囲に属するかどうかを判定する。
// true の場合は 1 を返し、それ以外の場合は 0 を返す。
atom is_alphanumeric_ascii_code(code: i64) -> i64
    requires: code >= 0 && code <= 127;
    ensures: result == 0 || result == 1;
    body: {
        let numeric_result = is_numeric_ascii_code(code);
        let is_lower_alpha = if code >= 97 && code <= 122 { 1 } else { 0 };
        let is_upper_alpha = if code >= 65 && code <= 90 { 1 } else { 0 };

        if numeric_result == 1 || is_lower_alpha == 1 || is_upper_alpha == 1 { 1 } else { 0 }
    };
