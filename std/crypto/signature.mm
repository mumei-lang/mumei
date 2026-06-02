// =============================================================
// Mumei Standard Library: std.crypto.signature
// =============================================================
// 署名検証の FFI バックエンド契約を提供する。
// public_key/message/signature は Mumei runtime の Str ハンドルを i64 として扱う。

extern "Rust" {
    fn crypto_verify_signature(public_key: i64, message: i64, signature: i64) -> i64
        requires: public_key > 0 && message > 0 && signature > 0;
        ensures: result >= 0 && result <= 1;
}

// Ed25519 署名検証結果を 0/1 で返す。
// TRUSTED(FFI): Rust ed25519-dalek backend verifies 32-byte public keys and
// 64-byte signatures encoded as lowercase/uppercase hex strings.
atom verify_signature(public_key: i64, message: i64, signature: i64)
    requires: public_key > 0 && message > 0 && signature > 0;
    ensures: result >= 0 && result <= 1;
    body: {
        crypto_verify_signature(public_key, message, signature)
    }

// 署名整合性 witness。検証結果は常に boolean domain に収まる。
atom signature_integrity(public_key: i64, message: i64, signature: i64)
    requires: public_key > 0 && message > 0 && signature > 0;
    ensures: result >= 0 && result <= 1;
    body: {
        verify_signature(public_key, message, signature)
    }

// 鍵・メッセージ・署名 handle の安全性を確認する。
atom signature_inputs_valid(public_key: i64, message: i64, signature: i64)
    requires: public_key > 0 && message > 0 && signature > 0;
    ensures: result == 1;
    body: {
        1
    }
