enum Mixed {
    Text(value: Str),
    Number(value: f64)
}

atom array_read(xs: [Mixed]) -> i64
requires: len(xs) > 0;
ensures: true;
body: match xs[0] {
    Mixed::Text(_) => 0,
    Mixed::Number(_) => 1
};
