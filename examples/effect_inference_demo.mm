// Effect Inference Demo: Demonstrates effect inference from call graph
//
// When running `mumei infer-effects`, the tool analyzes call graphs
// to determine which effects each atom needs.

effect FileWrite;
effect ConsoleOut;

/// Low-level atom with FileWrite effect
atom write_file(x: i64) -> i64
  effects: [FileWrite];
  requires: x >= 0;
  ensures: result == x;
  body: x;

/// Low-level atom with ConsoleOut effect
atom print_msg(x: i64) -> i64
  effects: [ConsoleOut];
  requires: x >= 0;
  ensures: result == x;
  body: x;

/// This atom calls write_file and print_msg,
/// so it should declare both FileWrite and ConsoleOut.
/// Effect inference will suggest adding missing effects.
atom process(x: i64) -> i64
  effects: [FileWrite, ConsoleOut];
  requires: x >= 0;
  ensures: result == x;
  body: x;
