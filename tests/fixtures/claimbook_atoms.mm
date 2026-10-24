atom verified_value(x: i64) -> i64
  requires: true;
  ensures: result == x;
  body: x;

trusted atom trusted_value(x: i64) -> i64
  requires: true;
  ensures: result == x;
  body: x;

atom failing_value(x: i64) -> i64
  requires: true;
  ensures: result == x + 1;
  body: x;
