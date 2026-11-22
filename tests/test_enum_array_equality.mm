enum C {
    R(v: i64),
    G
}

atom main() -> i64
requires: true;
ensures: true;
body: {
    let xs = [C::G, C::R(5)];
    let same_tag = xs[0] == C::G;
    let different_values = xs[0] != xs[1];
    let same_payload = C::R(5) == xs[1];
    if same_tag && different_values && same_payload {
        12
    } else {
        0
    }
};
