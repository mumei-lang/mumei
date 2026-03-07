// Effect Hierarchy Demo: Subtyping via parent relationships
//
// Demonstrates:
// - Effect hierarchy: HttpRead and TcpConnect are subtypes of Network
// - An atom declaring Network covers both HttpRead and TcpConnect
// - Subtype checking via is_subeffect()

effect Network;
effect HttpRead parent: Network;
effect TcpConnect parent: Network;

/// Atom that performs HTTP reading (subtype of Network)
atom fetch_data(x: i64) -> i64
  effects: [HttpRead];
  requires: x >= 0;
  ensures: result == x;
  body: x;

/// Atom that declares Network — covers all subtypes (HttpRead, TcpConnect)
/// Calls fetch_data (which requires HttpRead) to verify that Network covers HttpRead.
atom network_operation(x: i64) -> i64
  effects: [Network];
  requires: x >= 0;
  ensures: result == x;
  body: fetch_data(x);
