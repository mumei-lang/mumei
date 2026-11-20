// Composite-typed enum payloads `enum E { V(Point) }`
// (`pure-i64-composite/v18`). A payload field may name an admitted
// struct; the slot carries the struct's flattened leaves as one
// aggregate value and `match` binds it as a composite readable by
// field. Call pass-through keeps the whole payload intact.
//
// Constructor atoms (`Event::Move(Point { .. })`), whole-composite
// payload writes and enum equality are covered by the trusted-verifier
// unit tests in mumei-core; the general verifier still keeps composite
// payloads on the Int-tag path and cannot construct them in specs, so
// those atoms are out of this end-to-end fixture until the follow-on
// Int-tag constructor slice lands.

struct Point {
    x: i64,
    y: i64
}

enum Event {
    Ping,
    Move(pos: Point)
}

// A `match` binding on a composite payload binds the whole struct —
// `p.x`/`p.y` read its leaves.
atom step(e: Event) -> i64
requires: true;
ensures: result >= 0;
body: {
    match e {
        Event::Ping => 0,
        Event::Move(p) => if p.x + p.y >= 0 { p.x + p.y } else { 0 }
    }
};

// An enum with a composite payload passes through a call: the callee
// sees the same tag and the same flattened payload leaves.
atom hop(e: Event) -> i64
requires: true;
ensures: result >= 0;
body: {
    step(e)
};

// A bound composite payload reads a single leaf — the emitter lowers
// it to `extractvalue` on the aggregate slot.
atom read_x(e: Event) -> i64
requires: true;
ensures: result == result;
body: {
    match e {
        Event::Move(p) => p.x,
        _ => 0
    }
};

struct UPoint {
    x: u64,
    y: u64
}

enum UEvent {
    UPing,
    UMove(pos: UPoint)
}

// A `u64` payload leaf compares unsigned — `p.x > 0` must not lower
// to a signed `icmp sgt`.
atom uread(e: UEvent) -> i64
requires: true;
ensures: result == result;
body: {
    match e {
        UEvent::UMove(p) => if p.x > 0 { 1 } else { 0 },
        _ => 0
    }
};
