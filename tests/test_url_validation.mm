// =============================================================
// tests/test_url_validation.mm — URL Validation E2E Test
// =============================================================
// Integration test for HTTPS URL enforcement (Plan 23).
// Usage: mumei check tests/test_url_validation.mm

effect SecureHttpGet(url: Str) where starts_with(url, "https://");
effect SecureHttpPost(url: Str) where starts_with(url, "https://");

// Test 1: Valid literal HTTPS URL for GET
atom test_https_get()
    effects: [SecureHttpGet(url)]
    requires: true;
    ensures: result >= 0;
    body: {
        perform SecureHttpGet.get("https://api.example.com/data");
        1
    };

// Test 2: Valid literal HTTPS URL for POST
atom test_https_post()
    effects: [SecureHttpPost(url)]
    requires: true;
    ensures: result >= 0;
    body: {
        perform SecureHttpPost.post("https://api.example.com/submit");
        1
    };

// Test 3: Variable URL with requires constraint
atom test_variable_url(api_url: Str)
    effects: [SecureHttpGet(url)]
    requires: starts_with(api_url, "https://");
    ensures: result >= 0;
    body: {
        perform SecureHttpGet.get(api_url);
        1
    };
