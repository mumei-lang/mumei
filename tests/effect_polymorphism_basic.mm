effect FileWrite;

atom writer(x: i64)
    effects: [FileWrite];
    requires: x >= 0;
    ensures: result >= 0;
    body: { perform FileWrite.write(x); x };

atom pipe<E: Effect>(f: atom_ref(i64) -> i64 with E)
    effects: [E];
    requires: true;
    ensures: true;
    body: call(f, 42);

atom main()
    effects: [FileWrite];
    requires: true;
    ensures: result >= 0;
    body: pipe<FileWrite>(atom_ref(writer));
