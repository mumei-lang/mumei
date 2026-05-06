# Trusted Atoms

Trusted atoms are reviewed contract boundaries whose bodies are delegated to the runtime or an external proof backend. Z3 still checks each declared contract for consistency at call sites, but it cannot inspect the Rust FFI implementation behind these atoms.

## FFI trusted atom inventory

### `std/json.mm`

Backed by Rust `serde_json` value parsing, construction, query, and handle management:

- `parse`
- `stringify`
- `get`
- `get_int`
- `get_str`
- `get_bool`
- `array_len`
- `array_get`
- `is_null`
- `is_object`
- `is_array`
- `object_new`
- `object_set`
- `array_new`
- `array_push`
- `from_int`
- `from_str`
- `from_bool`
- `free`
- `str_free`

### `std/http.mm`

Backed by Rust `reqwest` HTTP client calls and response handle management:

- `get`
- `post`
- `put`
- `delete`
- `status`
- `body`
- `body_json`
- `header_get`
- `header_set`
- `is_ok`
- `is_error`
- `free`

### `std/http_secure.mm`

Backed by the same Rust HTTP client backend as `std/http.mm`, with Mumei-side HTTPS effect constraints:

- `secure_get`
- `secure_post`
- `secure_put`
- `secure_delete`
- `status`
- `body`
- `is_ok`
- `free`

### `std/http_server.mm`

Backed by Rust HTTP server/socket operations, with Mumei-side temporal effect transitions:

- `bind_server`
- `listen_server`
- `accept_request`
- `send_response`

### `std/file.mm`

Backed by Rust `std::fs` file operations:

- `read_file`
- `write_file`
- `exists`
- `remove`

## Why these atoms are trusted

The FFI body runs outside the current Mumei MIR/Z3 verification pipeline. Mumei can verify that callers satisfy `requires`, that declared `effects` are used consistently, and that consumers rely only on the declared `ensures`. It cannot yet prove that `serde_json`, `reqwest`, `std::fs`, or the handle table implementation always satisfies the declared postconditions.

Trusted status therefore means:

1. The contract is explicitly reviewed.
2. The runtime backend owns the body semantics.
3. Z3 verifies the Mumei-facing contract boundary, not the delegated implementation.

## Reduction roadmap

1. **FFI contract test harness**: generate runtime property tests from `requires`/`ensures` clauses and execute them against the Rust backend.
2. **FFI verification framework**: model handle tables, error paths, and resource lifetimes in a backend-specific proof layer.
3. **Lean contract witnesses**: export FFI contract obligations into Lean for high-value atoms whose semantics can be modeled independently.
4. **Typed resource handles**: replace raw `i64` handles with refined handle types so more validity constraints become statically checkable.
5. **Trusted budget tracking**: keep `MUMEI_TRUSTED_CREDIT` explicit in stdlib health metrics and reduce the credit as verified replacements become available.
