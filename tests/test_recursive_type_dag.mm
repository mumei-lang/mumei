enum E {
    Empty,
    Number(value: i64)
}

struct Pair {
    left: E,
    right: E
}

atom read_pair(pair: Pair) -> i64
requires: true;
ensures: result == 0;
body: {
    0
};
