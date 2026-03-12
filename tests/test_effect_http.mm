// =============================================================
// Test: HTTP effects — should pass verification
// =============================================================
// Network effect declarations with perform expressions.

effect HttpGet;
effect HttpPost;

atom fetch_data(url: i64) -> i64
  effects: [HttpGet];
  requires: url >= 0;
  ensures: result >= 0;
  body: {
    perform HttpGet.get(url);
    url
  }

atom post_data(url: i64, data: i64) -> i64
  effects: [HttpPost];
  requires: url >= 0 && data >= 0;
  ensures: result >= 0;
  body: {
    perform HttpPost.post(url);
    data
  }
