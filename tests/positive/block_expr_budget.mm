trusted atom passthrough(a: i64) -> i64
requires: true;
ensures: result == a;
body: a;

atom budget(x: i64) -> i64
requires: true;
ensures: result == x + 40;
body: passthrough({
    let a1 = x + 1;
    let a2 = a1 + 1;
    let a3 = a2 + 1;
    let a4 = a3 + 1;
    let a5 = a4 + 1;
    let a6 = a5 + 1;
    let a7 = a6 + 1;
    let a8 = a7 + 1;
    let a9 = a8 + 1;
    let a10 = a9 + 1;
    let a11 = a10 + 1;
    let a12 = a11 + 1;
    let a13 = a12 + 1;
    let a14 = a13 + 1;
    let a15 = a14 + 1;
    let a16 = a15 + 1;
    let a17 = a16 + 1;
    let a18 = a17 + 1;
    let a19 = a18 + 1;
    let a20 = a19 + 1;
    let a21 = a20 + 1;
    let a22 = a21 + 1;
    let a23 = a22 + 1;
    let a24 = a23 + 1;
    let a25 = a24 + 1;
    let a26 = a25 + 1;
    let a27 = a26 + 1;
    let a28 = a27 + 1;
    let a29 = a28 + 1;
    let a30 = a29 + 1;
    let a31 = a30 + 1;
    let a32 = a31 + 1;
    let a33 = a32 + 1;
    let a34 = a33 + 1;
    let a35 = a34 + 1;
    let a36 = a35 + 1;
    let a37 = a36 + 1;
    let a38 = a37 + 1;
    let a39 = a38 + 1;
    let a40 = a39 + 1;
    a40
});
