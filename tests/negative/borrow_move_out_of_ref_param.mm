trusted atom read_len(ref x: [i64])
requires: true;
ensures: true;
body: len(x);

atom bad(ref x: [i64])
requires: true;
ensures: true;
body: {
    let y = x;
    read_len(y)
};
