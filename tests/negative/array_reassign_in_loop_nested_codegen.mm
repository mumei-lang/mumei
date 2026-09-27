atom main() -> i64
requires: true;
ensures: true;
body: {
    let xs = [1, 2, 3];
    let i = 0;
    let acc = 0;
    while i < 2
    invariant: i >= 0 && i <= 2
    decreases: 2 - i
    {
        acc = acc + (if i == 0 { xs = [9, 9]; 1 } else { 0 });
        i = i + 1;
    }
    acc + len(xs)
}
