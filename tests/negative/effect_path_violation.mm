// Negative test: Effect parameter violates path constraint
// Expected: Constant folding check rejects path outside allowed prefix

effect FileWrite;

// Attempting to write to /etc/passwd should violate the constraint
// when FileWrite is defined with starts_with("/tmp/") constraint
atom write_system_file(x: i64) -> i64
  effects: [FileWrite];
  requires: x >= 0;
  ensures: result == x;
  body: x;
