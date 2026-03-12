// Negative test: HTTP effect not declared
// Expected: Verification failure — perform HttpGet without declaring effect

atom fetch_without_effect(url: i64) -> i64
  requires: url >= 0;
  ensures: result >= 0;
  body: {
    perform HttpGet.get(url);
    url
  }
