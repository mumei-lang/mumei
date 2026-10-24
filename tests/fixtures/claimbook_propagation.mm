trusted atom trusted_leaf() -> i64
  requires: true;
  ensures: result == 7;
  body: 7;

atom trusted_caller() -> i64
  requires: true;
  ensures: result == 7;
  body: trusted_leaf();

atom assumed_contract(x: i64) -> i64
  requires assume: x > 0;
  ensures: result > 0;
  body: x;
