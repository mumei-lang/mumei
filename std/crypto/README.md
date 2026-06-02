# std/crypto

`std/crypto` provides FFI-backed cryptographic contracts for Mumei programs.

## Modules

- `std/crypto/hash.mm`
  - `sha256(input: i64) -> i64`
  - `hash_eq(left: i64, right: i64) -> i64`
  - collision-resistance and determinism witnesses
- `std/crypto/hmac.mm`
  - `hmac_sha256(key: i64, message: i64) -> i64`
  - key validity and determinism witnesses
- `std/crypto/signature.mm`
  - `verify_signature(public_key: i64, message: i64, signature: i64) -> i64`
  - signature input and integrity witnesses

## FFI backend

The Rust backend stores Mumei `Str` values as runtime handles. Crypto atoms accept
those handles as `i64`, clone the associated string bytes, and return either:

- a new managed string handle containing a lowercase hex digest/tag, or
- a `0`/`1` verification result.

SHA-256 and HMAC-SHA256 are implemented with the existing Rust `sha2` crate.
Signature verification uses the Rust `ed25519-dalek` backend:

```text
public_key = 32-byte Ed25519 verifying key encoded as 64 hex chars
signature = 64-byte Ed25519 signature encoded as 128 hex chars
message = raw UTF-8 bytes stored in the Mumei Str handle
```
