// Negative test: Insufficient effect declaration with hierarchy
// Expected: HttpRead is declared but TcpConnect is also needed
//           (both are subtypes of Network, but siblings don't cover each other)

effect Network;
effect HttpRead parent: Network;
effect TcpConnect parent: Network;

atom http_reader(x: i64) -> i64
  effects: [HttpRead];
  requires: x >= 0;
  ensures: result == x;
  body: x;

atom tcp_connector(x: i64) -> i64
  effects: [TcpConnect];
  requires: x >= 0;
  ensures: result == x;
  body: x;

// Declares only HttpRead, but would need TcpConnect too if calling tcp_connector
// HttpRead and TcpConnect are siblings, not subtypes of each other
atom insufficient_effects(x: i64) -> i64
  effects: [HttpRead];
  requires: x >= 0;
  ensures: result == x;
  body: x;
