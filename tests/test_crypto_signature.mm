// =============================================================
// Crypto Signature/HMAC: E2E Verification Tests
// =============================================================

import "std/crypto/signature" as signature;
import "std/crypto/hmac" as hmac;

atom test_verify_signature_boolean(public_key: i64, message: i64, sig: i64)
    requires: public_key > 0 && message > 0 && sig > 0;
    ensures: result >= 0 && result <= 1;
    body: {
        signature::verify_signature(public_key, message, sig)
    }

atom test_signature_inputs_valid(public_key: i64, message: i64, sig: i64)
    requires: public_key > 0 && message > 0 && sig > 0;
    ensures: result == 1;
    body: {
        signature::signature_inputs_valid(public_key, message, sig)
    }

atom test_hmac_sha256_positive(key: i64, message: i64)
    requires: key > 0 && message > 0;
    ensures: result > 0;
    body: {
        hmac::hmac_sha256(key, message)
    }

atom test_hmac_key_valid(key: i64)
    requires: key > 0;
    ensures: result == 1;
    body: {
        hmac::hmac_key_is_valid(key)
    }
