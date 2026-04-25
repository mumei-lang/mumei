// =============================================================
// std/http_server.mm — Mumei HTTP Server Library
// =============================================================
// Provides verified HTTP server operations with temporal effect
// verification (stateful effect state machine).
//
// Usage:
//   import "std/http_server" as server;
//
//   let srv = server::bind_server("127.0.0.1:8080");
//   let ok  = server::listen_server(srv);
//   let req = server::accept_request(srv);
//   let ok  = server::send_response(req, 200, "Hello");
//
// The HttpServer stateful effect enforces correct operation ordering:
//   bind -> listen -> accept -> respond -> listen -> ...
//
// Temporal violations (e.g., respond before accept) are caught at compile time.

// --- HTTP Server Lifecycle State Machine ---
effect HttpServer
    states: [Init, Bound, Listening, Responding];
    initial: Init;
    transition bind: Init -> Bound;
    transition listen: Bound -> Listening;
    transition accept: Listening -> Responding;
    transition respond: Responding -> Listening;
    transition close: Listening -> Init;

// --- extern declarations: Rust FFI backend ---
extern "Rust" {
    fn http_server_bind(addr: Str) -> i64;
    fn http_server_listen(server_handle: i64) -> i64;
    fn http_server_accept(server_handle: i64) -> i64;
    fn http_request_path(req_handle: i64) -> Str;
    fn http_request_method(req_handle: i64) -> Str;
    fn http_server_respond(req_handle: i64, status: i64, body: Str) -> i64;
    fn http_server_free(server_handle: i64) -> i64;
    fn http_request_free(req_handle: i64) -> i64;
}

// =============================================================
// Public API: Server Operations
// =============================================================

// Bind a server to the given address. Returns a server handle (>0) or 0 on error.
// FFI-backed + stateful effect: contract enforced by Rust runtime.
trusted atom bind_server(addr: Str)
    effects: [HttpServer]
    requires: true;
    ensures: result >= 0;
    body: {
        perform HttpServer.bind(addr);
        http_server_bind(addr)
    }

// Start listening on a bound server. Transitions Bound → Listening.
// Since Rust's TcpListener::bind() already listens, this is a logical
// state transition that enables accept_request to be called.
// Returns 1 if server handle is valid, 0 otherwise.
// FFI-backed + stateful effect: contract enforced by Rust runtime.
trusted atom listen_server(server_handle: i64)
    effects: [HttpServer]
    requires: server_handle > 0;
    ensures: result >= 0;
    body: {
        perform HttpServer.listen(server_handle);
        http_server_listen(server_handle)
    }

// Accept an incoming request. Blocks until a connection arrives.
// Returns a request handle (>0) or 0 on error.
// FFI-backed + stateful effect: contract enforced by Rust runtime.
trusted atom accept_request(server_handle: i64)
    effects: [HttpServer]
    requires: server_handle > 0;
    ensures: result >= 0;
    body: {
        perform HttpServer.accept(server_handle);
        http_server_accept(server_handle)
    }

// Send an HTTP response with the given status code and body.
// Returns 1 on success, 0 on failure.
// FFI-backed + stateful effect: contract enforced by Rust runtime.
trusted atom send_response(req_handle: i64, status: i64, body: Str)
    effects: [HttpServer]
    requires: req_handle > 0 && status >= 100 && status < 600;
    ensures: result >= 0;
    body: {
        perform HttpServer.respond(req_handle);
        http_server_respond(req_handle, status, body)
    }
