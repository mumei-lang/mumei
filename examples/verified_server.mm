// examples/verified_server.mm
// A mathematically verified HTTP server that prevents directory traversal at compile time
//
// Usage:
//   mumei check examples/verified_server.mm
//
// This example combines:
//   1. Parameterized effects with compound constraints (SafeFileRead)
//   2. Stateful effects with temporal verification (HttpServer)
//   3. String concat propagation for path construction
//
// Expected results:
//   - serve_safe_file: PASS (user_id constrained, path safe)
//   - serve_unsafe_file: FAIL (user_id unconstrained, path may contain "..")
//   - double_respond: FAIL (temporal violation — respond called twice)

// Security policy: only /tmp/ paths, no ".." allowed
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

// SAFE file serving — passes verification
// The requires clause constrains req_path to not contain ".." or null bytes,
// which together with the "/tmp/public/" prefix satisfies SafeFileRead's constraint.
atom serve_safe_file(req_path: Str)
    effects: [SafeFileRead(path), HttpServer]
    requires: not_contains(req_path, "..") && not_contains(req_path, "\0");
    ensures: result >= 0;
    body: {
        let path = "/tmp/public/" + req_path;
        perform SafeFileRead.read(path);
        1
    }

// UNSAFE file serving — compile error (no constraints on req_path)
// Without requires constraining req_path, the user could pass "../../etc/passwd"
// which would construct "/tmp/public/../../etc/passwd" — a path traversal attack.
// Z3 finds this counterexample and rejects the program at compile time.
atom serve_unsafe_file(req_path: Str)
    effects: [SafeFileRead(path), HttpServer]
    requires: true;
    ensures: result >= 0;
    body: {
        let path = "/tmp/public/" + req_path;
        perform SafeFileRead.read(path);
        1
    }

// Double response — compile error (temporal violation)
// The HttpServer state machine only allows one respond per accept cycle:
//   bind → listen → accept → respond (OK, Responding → Listening)
//   → respond again (FAIL, no transition from Listening → Responding for respond)
atom double_respond(req: i64)
    effects: [HttpServer]
    requires: req > 0;
    ensures: result >= 0;
    body: {
        perform HttpServer.bind(req);
        perform HttpServer.listen(req);
        perform HttpServer.accept(req);
        perform HttpServer.respond(req);
        perform HttpServer.respond(req);
        1
    }
