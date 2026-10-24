trusted atom f(ref a: i64, ref b: i64)
requires: true;
ensures: true;
body: a;

trusted atom g(ref mut a: i64)
requires: true;
ensures: true;
body: a;

trusted atom h(ref a: i64)
requires: true;
ensures: true;
body: a;

atom mut_writer(ref mut x: i64)
requires: true;
ensures: true;
body: {
    x = x + 1;
    x
};

atom ok(x: i64)
requires: true;
ensures: true;
body: {
    f(x, x);
    g(x);
    h(x);
    x
};
