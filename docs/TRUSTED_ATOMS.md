# Trusted Atoms

Trusted atoms are reviewed contract boundaries whose bodies are delegated to the
runtime or an external proof backend. Z3 still checks each declared contract for
consistency at call sites, but it cannot inspect every external side effect or
proof backend implementation.

## Current inventory

As of `develop` on 2026-06-09, the historical FFI trusted-atom block has been
reduced from 48 to 0. `std/json.mm`, `std/http.mm`, `std/http_secure.mm`,
`std/http_server.mm`, and `std/file.mm` now expose verified Mumei wrappers over
their Rust FFI backends. Only the sorted-map quantified array invariant remains
trusted in the standard library.

| Module | Trusted atoms | Priority | Why trusted | Reduction path |
|---|---:|---|---|---|
| `std/container/sorted_map.mm` | 1 | Medium | `sorted_map_insert` relies on quantified array-store preservation (`keys[map_len] = key`) across a sortedness invariant. | Extend array-store tracking for append-at-end patterns and lower bounded `forall` into a Z3 decidable fragment with explicit index ranges. |

### FFI atoms with contract-test coverage

The FFI contract harness scans 52 FFI-facing atoms across the runtime-backed
modules below. All listed modules now expose verified wrappers and remain
covered by generated contract tests.

| Module | FFI atoms tested | Trusted remaining | Runtime backend |
|---|---:|---:|---|
| `std/json.mm` | 20 | 0 | `serde_json` value parsing/construction/query/handle management |
| `std/http.mm` | 12 | 0 | `reqwest` HTTP client calls and response handles |
| `std/http_secure.mm` | 8 | 0 | HTTPS-constrained `reqwest` client wrappers |
| `std/http_server.mm` | 4 | 0 | `std::net::TcpListener`/request-response handles |
| `std/file.mm` | 4 | 0 | `std::fs` file operations |
| `std/crypto/{hash,hmac,signature}.mm` | 4 | 0 | cryptographic Rust helpers |

## Why remaining atoms are trusted

Trusted status means:

1. The contract is explicitly reviewed.
2. The runtime or proof backend owns semantics that are not yet represented as
   pure Mumei MIR.
3. Z3 verifies the Mumei-facing contract boundary and callers, not the delegated
   implementation internals.

The remaining sorted-map atom combines quantified array invariants with
mutation. It is better reduced by improving the verifier fragment rather than
by hard-coding proof assumptions.

## Reduction roadmap

### Priority 1 — complete: eliminate the 4 `std/http_server.mm` FFI trusted atoms

- Pure witness layer:
  - `server_bound(handle)`, `server_listening(handle)`, `request_live(handle)`.
  - Effects transition these witnesses while the Rust socket call remains an
    extern implementation detail.
- Raw `i64` handles are wrapped by refined validity predicates:
  - `server_handle > 0 && server_bound(server_handle)`.
  - `req_handle > 0 && request_live(req_handle)`.
- Wrapper bodies are verified `atom` declarations over decidable temporal
  witness transitions.
- Generated contract tests remain runtime regression coverage for bind/listen,
  pending-client accept, and response boundary statuses (`100`, `599`).

### Priority 2 — eliminate `std/container/sorted_map.mm::sorted_map_insert`

- Extend array-store tracking from scalar index facts to append-at-end updates:
  - pre: `forall(i, 0, map_len - 1, keys[i] <= keys[i + 1])`
  - write: `keys[map_len] = key`
  - side condition: `forall(i, 0, map_len, keys[i] <= key)`
  - post: `forall(i, 0, map_len, keys[i] <= keys[i + 1])`
- Lower bounded quantifiers into finite integer ranges when both bounds are
  linear expressions over atom parameters.
- Add regression certificates for append, remove-tail, and no-op removal cases.

### Priority 3 — keep FFI contract harness above 80% coverage

`scripts/ffi_contract_test_gen.py --report` now reports:

- total FFI atoms scanned,
- trusted FFI atoms scanned,
- generated/skipped test counts,
- contract coverage percentage,
- `Coverage status: PASS|FAIL` against the 80% target,
- per-module generated/skipped/trusted counts.

CI enforces the coverage target before running `cargo test -p mumei-ffi-tests`.
The generator also emits deterministic edge-case tests for boundary values,
missing files, invalid JSON, HTTP header/status paths, HTTPS error URLs, and
HTTP server response status boundaries in addition to proptest strategies.

### Priority 4 — track progress in stdlib health metrics

`docs/STDLIB_METRICS.md` explicitly lists total trusted atoms, trusted modules,
per-module trusted counts, and historical trusted counts. This makes the
remaining budget visible even when the weighted health score is high.
