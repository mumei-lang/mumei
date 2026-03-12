effect FileWrite;
effect Network;

atom net_call(x: i64)
    effects: [Network];
    requires: true;
    ensures: true;
    body: { perform Network.get(x); x };

atom pipe<E: Effect>(f: atom_ref(i64) -> i64 with E)
    effects: [E];
    requires: true;
    ensures: true;
    body: call(f, 42);

atom main()
    effects: [FileWrite];
    requires: true;
    ensures: true;
    body: pipe<Network>(atom_ref(net_call));
