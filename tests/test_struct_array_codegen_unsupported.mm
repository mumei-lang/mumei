enum E {
    A,
    B(next: E)
}

struct Bad {
    value: E
}

atom main(values: [Bad]) -> i64
requires: len(values) > 0;
ensures: true;
body: values[0].value.tag;
