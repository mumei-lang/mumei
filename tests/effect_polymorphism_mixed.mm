effect Network;

atom apply<T: Comparable, E: Effect>(
    x: i64,
    f: atom_ref(i64) -> i64 with E
)
    effects: [E];
    requires: x >= 0;
    ensures: true;
    body: call(f, x);

atom net_fn(x: i64)
    effects: [Network];
    requires: x >= 0;
    ensures: true;
    body: { perform Network.get(x); x };

atom main()
    effects: [Network];
    requires: true;
    ensures: true;
    body: apply<i64, Network>(42, atom_ref(net_fn));
