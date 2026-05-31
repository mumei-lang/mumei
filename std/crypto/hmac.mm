// =============================================================
// Mumei Standard Library: std.crypto.hmac
// =============================================================
// HMAC-SHA256 の FFI バックエンド契約を提供する。
// key/message/result は Mumei runtime の Str ハンドルを i64 として扱う。

extern "Rust" {
    fn crypto_hmac_sha256(key: i64, message: i64) -> i64
        requires: key > 0 && message > 0;
        ensures: result > 0;
    fn crypto_hash_eq(left: i64, right: i64) -> i64
        requires: left > 0 && right > 0;
        ensures: result >= 0 && result <= 1;
}

// HMAC-SHA256 tag を hex Str ハンドルとして返す。
// TRUSTED(FFI): Rust sha2 backend implements RFC 2104 HMAC construction.
atom hmac_sha256(key: i64, message: i64)
    requires: key > 0 && message > 0;
    ensures: result > 0;
    body: {
        crypto_hmac_sha256(key, message)
    }

// キー安全性の最小契約。0 は null/invalid handle として拒否する。
atom hmac_key_is_valid(key: i64)
    requires: key > 0;
    ensures: result == 1;
    body: {
        1
    }

// 同じ key/message は同じ tag になる決定性 witness。
atom hmac_sha256_deterministic(key: i64, message: i64)
    requires: key > 0 && message > 0;
    ensures: result >= 0 && result <= 1;
    body: {
        let tag_a = hmac_sha256(key, message);
        let tag_b = hmac_sha256(key, message);
        if crypto_hash_eq(tag_a, tag_b) == 1 { 1 } else { 0 }
    }
