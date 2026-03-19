// =============================================================
// Plan 10/11: FFI Backend Module
// =============================================================
// Provides Rust-side #[no_mangle] implementations for std/json.mm
// and std/http.mm extern declarations.

pub mod http;
pub mod http_server;
pub mod json;
