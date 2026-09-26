trusted atom take(x: [i64])
consume x;
requires: true;
ensures: true;
body: len(x);

trusted atom read(ref x: [i64])
requires: true;
ensures: true;
body: len(x);

atom bad(x: [i64])
requires: true;
ensures: true;
body: {
    take(x);
    read(x)
};
