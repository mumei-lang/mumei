effect FileRead;
effect FileWrite;

atom reader(x: i64)
    effects: [FileRead];
    requires: true;
    ensures: true;
    body: { perform FileRead.read(x); x };

atom writer(x: i64)
    effects: [FileWrite];
    requires: true;
    ensures: true;
    body: { perform FileWrite.write(x); x };

atom transform<E1: Effect, E2: Effect>(
    r: atom_ref(i64) -> i64 with E1,
    w: atom_ref(i64) -> i64 with E2
)
    effects: [E1, E2];
    requires: true;
    ensures: true;
    body: {
        let x = call(r, 1);
        call(w, x)
    };

atom main()
    effects: [FileRead, FileWrite];
    requires: true;
    ensures: true;
    body: transform<FileRead, FileWrite>(atom_ref(reader), atom_ref(writer));
