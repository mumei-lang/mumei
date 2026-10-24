atom main() -> i64
requires: true;
ensures: true;
body: {
    let x = 1;
    let g = { x = 2; let h = |n: i64| -> i64 { n + x }; h };
    g(1)
};
