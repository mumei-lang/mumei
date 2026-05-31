// =============================================================
// Crypto Hash: E2E Verification Tests
// =============================================================

import "std/crypto/hash" as crypto_hash;

atom test_sha256_positive(input: i64)
    requires: input > 0;
    ensures: result > 0;
    body: {
        crypto_hash::sha256(input)
    }

atom test_sha256_deterministic(input: i64)
    requires: input > 0;
    ensures: result >= 0 && result <= 1;
    body: {
        crypto_hash::sha256_deterministic(input)
    }

atom test_sha256_collision_witness(input_a: i64, input_b: i64)
    requires: input_a > 0 && input_b > 0 && input_a != input_b;
    ensures: result >= 0 && result <= 1;
    body: {
        crypto_hash::sha256_collision_resistant(input_a, input_b)
    }
