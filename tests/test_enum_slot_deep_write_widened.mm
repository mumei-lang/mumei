enum Chan {
    Tcp(port: i64),
    Udp
}

enum WideOuter {
    Pad(value: i64),
    Wrap(c: Chan)
}

atom reject_widened_enum_slot(o: WideOuter, v: i64) -> i64
requires: match o {
    WideOuter::Wrap(c) => match c {
        Chan::Tcp(_) => true,
        _ => false
    },
    _ => false
};
ensures: true;
body: o.c.port = v;
