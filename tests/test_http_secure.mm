// =============================================================
// tests/test_http_secure.mm — Secure HTTP E2E Test
// =============================================================
// Integration test for HTTPS-only HTTP client (std/http_secure.mm).
// Follows the pattern of tests/test_url_validation.mm.
// Usage: mumei check tests/test_http_secure.mm

effect SecureHttpGet(url: Str) where starts_with(url, "https://");
effect SecureHttpPost(url: Str) where starts_with(url, "https://");

// Test 1: HTTPS literal URL for GET
atom test_secure_get_literal()
    effects: [SecureHttpGet(url)]
    requires: true;
    ensures: result >= 0;
    body: {
        perform SecureHttpGet.get("https://api.example.com/data");
        1
    };

// Test 2: HTTPS literal URL for POST
atom test_secure_post_literal()
    effects: [SecureHttpPost(url)]
    requires: true;
    ensures: result >= 0;
    body: {
        perform SecureHttpPost.post("https://api.example.com/submit");
        1
    };

// Test 3: Variable URL with requires constraint
atom test_secure_get_variable(api_url: Str)
    effects: [SecureHttpGet(url)]
    requires: starts_with(api_url, "https://");
    ensures: result >= 0;
    body: {
        perform SecureHttpGet.get(api_url);
        1
    };
