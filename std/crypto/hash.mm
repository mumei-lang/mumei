// =============================================================
// Mumei Standard Library: std.crypto.hash
// =============================================================
// SHA-256 の FFI バックエンド契約と検証用 witness を提供する。
// 入力・出力は Mumei runtime の Str ハンドルを i64 として扱う。

extern "Rust" {
    fn crypto_sha256(input: i64) -> i64
        requires: input > 0;
        ensures: result > 0;
    fn crypto_hash_eq(left: i64, right: i64) -> i64
        requires: left > 0 && right > 0;
        ensures: result >= 0 && result <= 1;
}

// SHA-256 digest を hex Str ハンドルとして返す。
// TRUSTED(FFI): Rust sha2 backend computes the 32-byte digest and returns
// its 64-byte lowercase hex representation.
atom sha256(input: i64)
    requires: input > 0;
    ensures: result > 0;
    body: {
        crypto_sha256(input)
    }

// digest equality を 0/1 witness として公開する。
// TRUSTED(FFI): Runtime compares canonical digest strings.
atom hash_eq(left: i64, right: i64)
    requires: left > 0 && right > 0;
    ensures: result >= 0 && result <= 1;
    body: {
        crypto_hash_eq(left, right)
    }

// SHA-256 の衝突耐性契約 witness。
// 異なる入力から同じ digest が観測された場合のみ 0 を返す。
atom sha256_collision_resistant(input_a: i64, input_b: i64)
    requires: input_a > 0 && input_b > 0 && input_a != input_b;
    ensures: result >= 0 && result <= 1;
    body: {
        let digest_a = sha256(input_a);
        let digest_b = sha256(input_b);
        if hash_eq(digest_a, digest_b) == 1 { 0 } else { 1 }
    }

// 同じ入力は同じ digest になる決定性 witness。
atom sha256_deterministic(input: i64)
    requires: input > 0;
    ensures: result >= 0 && result <= 1;
    body: {
        let digest_a = sha256(input);
        let digest_b = sha256(input);
        if hash_eq(digest_a, digest_b) == 1 { 1 } else { 0 }
    }
