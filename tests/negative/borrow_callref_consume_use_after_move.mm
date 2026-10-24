trusted atom take(y: [i64])
consume y;
requires: true;
ensures: true;
body: len(y);

atom bad(x: [i64])
requires: true;
ensures: true;
body: {
    let n = call(atom_ref(take), x);
    len(x)
};
