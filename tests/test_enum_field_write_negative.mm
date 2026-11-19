// Rejected enum payload writes.

enum Chan {
    Tcp(port: i64),
    Udp
}

enum Two {
    A(x: i64),
    B(x: i64)
}

// Wrong tag: `c` holds `Udp`, so `is-Tcp(c)` is refutable.
atom bad_wrong_tag(p: i64) -> Chan
requires: true;
ensures: true;
body: {
    let c = Chan::Udp;
    c.port = p;
    c
};

// Unknown tag: a param outside match narrowing carries no proof.
atom bad_unknown_tag(c: Chan, p: i64) -> Chan
requires: true;
ensures: true;
body: {
    c.port = p;
    c
};

// Ambiguous payload name: `x` is a slot of both `A` and `B` — there
// is no single slot the write can land on.
atom bad_ambiguous(e: Two, v: i64) -> Two
requires: true;
ensures: true;
body: {
    match e {
        Two::A(x) => { e.x = v; e }
        _ => e
    }
};

// Unknown field: `missing` is no variant's payload.
atom bad_unknown_field(v: i64) -> Chan
requires: true;
ensures: true;
body: {
    let c = Chan::Tcp(0);
    c.missing = v;
    c
};

// Sort mismatch: a `bool` value cannot fill an `i64` slot.
atom bad_sort(v: bool) -> Chan
requires: true;
ensures: true;
body: {
    let c = Chan::Tcp(0);
    c.port = v;
    c
};

type PosPort = i64 where v > 0;
enum Refined { Good(x: PosPort), Blank }

// The written value violates the payload's refined-alias predicate —
// match-arm payload binding assumes it, so the write is rejected.
atom bad_refined_payload(r: Refined) -> Refined
requires: true;
ensures: result == r;
body: {
    match r {
        Refined::Good(x) => { r.x = 0 - 1; r }
        _ => r
    }
};
