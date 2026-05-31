use sha2::{Digest, Sha256};

fn handle_to_string(handle: i64) -> Option<String> {
    if handle <= 0 {
        return None;
    }
    super::json::mumei_str_clone(handle)
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex_digest(&hasher.finalize())
}

fn hmac_sha256_hex(key: &str, message: &str) -> String {
    const BLOCK_SIZE: usize = 64;
    let mut key_bytes = key.as_bytes().to_vec();
    if key_bytes.len() > BLOCK_SIZE {
        let mut hasher = Sha256::new();
        hasher.update(&key_bytes);
        key_bytes = hasher.finalize().to_vec();
    }
    key_bytes.resize(BLOCK_SIZE, 0);

    let mut inner_pad = [0x36_u8; BLOCK_SIZE];
    let mut outer_pad = [0x5c_u8; BLOCK_SIZE];
    for (i, key_byte) in key_bytes.iter().enumerate() {
        inner_pad[i] ^= key_byte;
        outer_pad[i] ^= key_byte;
    }

    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message.as_bytes());
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    hex_digest(&outer.finalize())
}

#[no_mangle]
pub extern "C" fn crypto_sha256(input: i64) -> i64 {
    let Some(input) = handle_to_string(input) else {
        return 0;
    };
    super::json::mumei_str_alloc_internal(&sha256_hex(&input))
}

#[no_mangle]
pub extern "C" fn crypto_hash_eq(left: i64, right: i64) -> i64 {
    let Some(left) = handle_to_string(left) else {
        return 0;
    };
    let Some(right) = handle_to_string(right) else {
        return 0;
    };
    if left == right {
        1
    } else {
        0
    }
}

#[no_mangle]
pub extern "C" fn crypto_hmac_sha256(key: i64, message: i64) -> i64 {
    let Some(key) = handle_to_string(key) else {
        return 0;
    };
    let Some(message) = handle_to_string(message) else {
        return 0;
    };
    super::json::mumei_str_alloc_internal(&hmac_sha256_hex(&key, &message))
}

#[no_mangle]
pub extern "C" fn crypto_verify_signature(public_key: i64, message: i64, signature: i64) -> i64 {
    let Some(public_key) = handle_to_string(public_key) else {
        return 0;
    };
    let Some(message) = handle_to_string(message) else {
        return 0;
    };
    let Some(signature) = handle_to_string(signature) else {
        return 0;
    };
    let expected = sha256_hex(&format!("{public_key}:{message}"));
    if signature == expected {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        );
    }

    #[test]
    fn hmac_sha256_matches_known_vector() {
        assert_eq!(
            hmac_sha256_hex("key", "The quick brown fox jumps over the lazy dog"),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8",
        );
    }
}
