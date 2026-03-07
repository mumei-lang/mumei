// Negative test: Declared effects don't match inferred effects
// Expected: Effect inference suggests adding missing effects

effect FileWrite;
effect ConsoleOut;
effect Network;

atom file_writer(x: i64) -> i64
  effects: [FileWrite];
  requires: x >= 0;
  ensures: result == x;
  body: x;

atom network_caller(x: i64) -> i64
  effects: [Network];
  requires: x >= 0;
  ensures: result == x;
  body: x;

// Declares only ConsoleOut but calls both file_writer and network_caller
// Expected: effect propagation violation (FileWrite and Network not declared)
atom mismatched(x: i64) -> i64
  effects: [ConsoleOut];
  requires: x >= 0;
  ensures: result == x;
  body: { let a = file_writer(x); network_caller(a) };
