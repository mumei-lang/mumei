// =============================================================
// Verified Server: E2E Verification Tests
// =============================================================
// Tests for the integrated verified HTTP server combining
// path safety (parameterized effects) and temporal effects.
//
// Usage:
//   mumei check tests/test_verified_server.mm

// Security policy effect
effect SafeFileRead(path: Str) where starts_with(path, "/tmp/") && not_contains(path, "..");

// Server lifecycle state machine
effect HttpServer
    states: [Init, Bound, Listening, Responding];
    initial: Init;
    transition bind: Init -> Bound;
    transition listen: Bound -> Listening;
    transition accept: Listening -> Responding;
    transition respond: Responding -> Listening;
    transition close: Listening -> Init;

// --- Test 1: Safe file serving with constrained path ---
atom test_serve_safe(req_path: Str)
    effects: [SafeFileRead(path)]
    requires: not_contains(req_path, "..") && not_contains(req_path, "\0");
    ensures: result >= 0;
    body: {
        let path = "/tmp/public/" + req_path;
        perform SafeFileRead.read(path);
        1
    }

// --- Test 2: Server bind operation ---
atom test_server_bind(addr: Str)
    effects: [HttpServer]
    requires: true;
    ensures: result >= 0;
    body: {
        perform HttpServer.bind(addr);
        1
    }

// --- Test 3: Combined path safety and server effects ---
// Exercises the full lifecycle: bind → listen → accept → read file → respond
atom test_combined(req_path: Str)
    effects: [SafeFileRead(path), HttpServer]
    requires: not_contains(req_path, "..") && not_contains(req_path, "\0");
    ensures: result >= 0;
    body: {
        perform HttpServer.bind(1);
        perform HttpServer.listen(1);
        perform HttpServer.accept(1);
        let path = "/tmp/public/" + req_path;
        perform SafeFileRead.read(path);
        perform HttpServer.respond(1);
        1
    }
