// =============================================================
// HTTP E2E Demo: Verified HTTP Client with FFI
// =============================================================
// Demonstrates mumei's verified FFI layer for real-world HTTP usage.
// Each atom has formal contracts (requires/ensures) that Z3 verifies
// at compile time, ensuring safety properties hold before execution.
//
// This demo shows:
//   1. Extern function declarations with verified contracts
//   2. Safe wrappers that satisfy extern preconditions
//   3. Unsafe callers that violate contracts (caught at compile time)
//   4. Composition of verified HTTP operations
//
// Usage:
//   mumei check examples/http_e2e_demo.mm
//   mumei verify examples/http_e2e_demo.mm
//
// Expected results:
//   - fetch_user_safe: PASS (url precondition satisfied)
//   - fetch_user_unsafe: FAIL (url may be empty)
//   - fetch_and_check: PASS (status code contract propagated)

import "std/http" as http;
import "std/json" as json;

// --- Safe HTTP GET: URL is guaranteed non-empty ---
atom fetch_user_safe(username: Str)
    requires: len(username) > 0;
    ensures: result >= 0;
    body: {
        let url = "https://api.github.com/users/" + username;
        let response = http::get(url);
        http::status(response)
    }

// --- Unsafe HTTP GET: no constraint on username ---
// Should FAIL verification: len(username) could be 0,
// producing url = "https://api.github.com/users/" + "" which
// violates the semantic expectation of a valid API endpoint.
atom fetch_user_unsafe(username: Str)
    requires: true;
    ensures: result >= 0;
    body: {
        let response = http::get(username);
        http::status(response)
    }

// --- Safe fetch + JSON parse pipeline ---
atom fetch_and_parse_user(username: Str)
    requires: len(username) > 0;
    ensures: result >= 0;
    body: {
        let url = "https://api.github.com/users/" + username;
        let response = http::get(url);
        let data = http::body_json(response);
        data
    }

// --- Status code validation ---
atom fetch_and_check(username: Str)
    requires: len(username) > 0;
    ensures: result >= 0 && result <= 1;
    body: {
        let url = "https://api.github.com/users/" + username;
        let response = http::get(url);
        http::is_ok(response)
    }

// --- Composition: fetch two users and aggregate ---
atom compare_users(user1: Str, user2: Str)
    requires: len(user1) > 0 && len(user2) > 0;
    ensures: result >= 0;
    body: {
        let url1 = "https://api.github.com/users/" + user1;
        let url2 = "https://api.github.com/users/" + user2;
        let r1 = http::get(url1);
        let r2 = http::get(url2);
        let s1 = http::status(r1);
        let s2 = http::status(r2);
        s1 + s2
    }
