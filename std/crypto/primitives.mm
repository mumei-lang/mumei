// =============================================================
// std/crypto/primitives.mm — deterministic forge output
// =============================================================
// 明示 body を持つ forge task から決定的に生成した検証対象。
// LLM 呼び出しなしで forge できる構造的契約のみを含む。
//
// Usage:
//   import "crypto/primitives" as primitives;

// --- is_valid_key_len ---
atom is_valid_key_len(key_len: i64, min_len: i64, max_len: i64)
    requires: min_len >= 0 && max_len >= min_len;
    ensures: result == 0 || result == 1;
    body: {
        if key_len >= min_len { if key_len <= max_len { 1 } else { 0 } } else { 0 }
    };

// --- is_valid_nonce_len ---
atom is_valid_nonce_len(nonce_len: i64, expected_len: i64)
    requires: nonce_len >= 0 && expected_len > 0;
    ensures: result == 0 || result == 1;
    body: {
        if nonce_len == expected_len { 1 } else { 0 }
    };

// --- constant_time_eq_flag ---
atom constant_time_eq_flag(left: i64, right: i64)
    requires: left >= 0 && right >= 0;
    ensures: result == 0 || result == 1;
    body: {
        if left == right { 1 } else { 0 }
    };

// --- digest_len_ok ---
atom digest_len_ok(digest_len: i64, max_digest_len: i64)
    requires: digest_len >= 0 && max_digest_len > 0;
    ensures: result == 0 || result == 1;
    body: {
        if digest_len > 0 { if digest_len <= max_digest_len { 1 } else { 0 } } else { 0 }
    };
