// =============================================================
// Secure HTTP: Compile-Time HTTPS Enforcement
// =============================================================
// Demonstrates HTTPS-only URL validation using starts_with().

effect SecureHttpGet(url: Str) where starts_with(url, "https://");

atom fetch_api()
    effects: [SecureHttpGet(url)]
    requires: true;
    ensures: result >= 0;
    body: {
        perform SecureHttpGet.get("https://api.example.com/users");
        1
    };

atom fetch_variable(api_url: Str)
    effects: [SecureHttpGet(url)]
    requires: starts_with(api_url, "https://");
    ensures: result >= 0;
    body: {
        perform SecureHttpGet.get(api_url);
        1
    };
