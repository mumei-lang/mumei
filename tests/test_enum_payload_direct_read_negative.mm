enum Chan {
    Tcp(port: i64, keep: i64),
    Udp
}

enum Outer {
    Wrap(c: Chan, marker: i64),
    Nil
}

enum Positional {
    Tcp(i64, i64),
    Udp
}

enum Ambiguous {
    Left(port: i64),
    Right(port: i64)
}

atom bad_tag_read(v: i64) -> i64
requires: v != 0;
ensures: result == 0;
body: {
    let e = Chan::Tcp(v, 9);
    e.port
};

atom bad_sibling_read(v: i64) -> i64
requires: v != 9;
ensures: result == 9;
body: {
    let e = Chan::Tcp(v, 9);
    e.port
};

atom bad_unnarrowed_parameter(e: Chan, v: i64) -> i64
requires: true;
ensures: result == v;
body: e.port;

atom bad_wrong_variant_path(e: Chan, v: i64) -> i64
requires: match e {
    Chan::Udp => true,
    _ => false
};
ensures: result == v;
body: e.port;

atom bad_requires_without_tag_does_not_narrow(e: Chan, v: i64) -> i64
requires: e.port == v;
ensures: result == v;
body: e.port;

atom bad_wrong_variant_requires_does_not_bind_payload(e: Chan, v: i64) -> i64
requires: e.tag == 1 && e.port == v;
ensures: result == v;
body: e.port;

atom bad_outer_wrong_variant_path(o: Outer, v: i64) -> i64
requires: match o {
    Outer::Nil => true,
    _ => false
} && v == 0;
ensures: result == v;
body: o.c.port;

atom bad_unknown_field(e: Chan, v: i64) -> i64
requires: match e {
    Chan::Tcp(port, _) => port == v,
    _ => false
};
ensures: result == v;
body: e.missing;

atom bad_positional_payload(v: i64) -> i64
requires: true;
ensures: result == v;
body: {
    let e = Positional::Tcp(v, 9);
    e.port
};

atom bad_ambiguous_owner(e: Ambiguous, v: i64) -> i64
requires: match e {
    Ambiguous::Left(port) => port == v,
    _ => false
};
ensures: result == v;
body: e.port;
