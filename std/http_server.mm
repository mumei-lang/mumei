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

// --- Pure witness layer for the temporal state machine ---
type ServerHandle = i64 where server_handle > 0;
type RequestHandle = i64 where req_handle > 0;

// --- extern declarations: Rust FFI backend ---
extern "Rust" {
    fn http_server_bind(addr: Str) -> i64
        requires: contains(addr, ":") && not_contains(addr, "\n") && not_contains(addr, "\r");
        ensures: result >= 0 && (result == 0 || server_bound(result));
    fn http_server_listen(server_handle: ServerHandle) -> i64
        requires: server_handle > 0 && server_bound(server_handle);
        ensures: result >= 0 && result <= 1 && server_listening(server_handle);
    fn http_server_accept(server_handle: ServerHandle) -> i64
        requires: server_handle > 0 && server_listening(server_handle);
        ensures: result >= 0 && (result == 0 || request_live(result));
    fn http_request_path(req_handle: RequestHandle) -> Str
        requires: req_handle > 0;
        ensures: true;
    fn http_request_method(req_handle: RequestHandle) -> Str
        requires: req_handle > 0;
        ensures: true;
    fn http_server_respond(req_handle: RequestHandle, status: i64, body: Str) -> i64
        requires: req_handle > 0 && request_live(req_handle) && status >= 100 && status <= 599;
        ensures: result >= 0 && result <= 1;
    fn http_server_free(server_handle: ServerHandle) -> i64
        requires: server_handle > 0;
        ensures: result >= 0 && result <= 1;
    fn http_request_free(req_handle: RequestHandle) -> i64
        requires: req_handle > 0;
        ensures: result >= 0 && result <= 1;
}

// =============================================================
// Public API: Server Operations
// =============================================================

// Bind a server to the given address and mint a bound-server witness.
atom bind_server(addr: Str)
    effects: [HttpServer]
    effect_pre: { HttpServer: Init };
    effect_post: { HttpServer: Bound };
    requires: contains(addr, ":") && not_contains(addr, "\n") && not_contains(addr, "\r");
    ensures: result >= 0 && (result == 0 || server_bound(result));
    body: {
        perform HttpServer.bind(addr);
        http_server_bind(addr)
    }

// Start listening on a bound server. Transitions Bound → Listening.
// Since Rust's TcpListener::bind() already listens, this is a logical
// state transition that enables accept_request to be called.
// Returns 1 if server handle is valid, 0 otherwise.
atom listen_server(server_handle: ServerHandle)
    effects: [HttpServer]
    effect_pre: { HttpServer: Bound };
    effect_post: { HttpServer: Listening };
    requires: server_handle > 0 && server_bound(server_handle);
    ensures: result >= 0 && result <= 1 && server_listening(server_handle);
    body: {
        perform HttpServer.listen(server_handle);
        http_server_listen(server_handle)
    }

// Accept an incoming request and mint a live-request witness.
atom accept_request(server_handle: ServerHandle)
    effects: [HttpServer]
    effect_pre: { HttpServer: Listening };
    effect_post: { HttpServer: Responding };
    requires: server_handle > 0 && server_listening(server_handle);
    ensures: result >= 0 && (result == 0 || request_live(result));
    body: {
        perform HttpServer.accept(server_handle);
        http_server_accept(server_handle)
    }

// Send an HTTP response with the given status code and body.
// Returns 1 on success, 0 on failure.
atom send_response(req_handle: RequestHandle, status: i64, body: Str)
    effects: [HttpServer]
    effect_pre: { HttpServer: Responding };
    effect_post: { HttpServer: Listening };
    requires: req_handle > 0 && request_live(req_handle) && status >= 100 && status <= 599;
    ensures: result >= 0 && result <= 1;
    body: {
        perform HttpServer.respond(req_handle);
        http_server_respond(req_handle, status, body)
    }
