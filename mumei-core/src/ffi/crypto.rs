use ed25519_dalek::{Signature, Verifier, VerifyingKey};
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

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode_hex_exact<const N: usize>(input: &str) -> Option<[u8; N]> {
    if input.len() != N * 2 {
        return None;
    }

    let mut out = [0u8; N];
    for (idx, chunk) in input.as_bytes().chunks_exact(2).enumerate() {
        let high = hex_nibble(chunk[0])?;
        let low = hex_nibble(chunk[1])?;
        out[idx] = (high << 4) | low;
    }
    Some(out)
}

fn verify_ed25519_signature(public_key: &str, message: &str, signature: &str) -> bool {
    let Some(public_key) = decode_hex_exact::<32>(public_key) else {
        return false;
    };
    let Some(signature) = decode_hex_exact::<64>(signature) else {
        return false;
    };

    let Ok(verifying_key) = VerifyingKey::from_bytes(&public_key) else {
        return false;
    };
    let signature = Signature::from_bytes(&signature);
    verifying_key.verify(message.as_bytes(), &signature).is_ok()
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
    if verify_ed25519_signature(&public_key, &message, &signature) {
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

    #[test]
    fn verifies_ed25519_rfc8032_empty_message_vector() {
        let public_key = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
        let signature = concat!(
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155",
            "5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
        );
        assert!(verify_ed25519_signature(public_key, "", signature));
        assert!(!verify_ed25519_signature(public_key, "tampered", signature));
    }
}
