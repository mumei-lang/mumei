// Negative: a forged collision witness cannot prove impossible equality.
import "std/crypto/hash" as crypto_hash;

atom forged_collision(input_a: i64, input_b: i64)
    requires: input_a > 0 && input_b > 0 && input_a != input_b;
    ensures: result == 1;
    body: {
        crypto_hash::sha256_collision_resistant(input_a, input_b)
    }
