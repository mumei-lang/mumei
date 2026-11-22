struct Box<T> {
    value: T
}

struct T {
    value: Box<i64>
}

atom read(value: T) -> i64
requires: true;
ensures: result == 0;
body: {
    0
};
