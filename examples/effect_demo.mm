// Effect System Demo: Basic effect declarations and usage
//
// Demonstrates:
// - Effect declarations (basic and with parent hierarchy)
// - Atom effect annotations
// - Pure atoms (no effects)

effect Network;
effect HttpRead parent: Network;
effect TcpConnect parent: Network;
effect FileWrite;
effect ConsoleOut;

/// Pure function: no effects required
atom add(x: i64, y: i64) -> i64
  requires: true;
  ensures: result == x + y;
  body: x + y;

/// Effectful atom: requires ConsoleOut
atom log_value(x: i64) -> i64
  effects: [ConsoleOut];
  requires: x >= 0;
  ensures: result == x;
  body: x;

/// Effectful atom: requires FileWrite and ConsoleOut
atom write_and_log(x: i64) -> i64
  effects: [FileWrite, ConsoleOut];
  requires: x >= 0;
  ensures: result == x;
  body: x;
