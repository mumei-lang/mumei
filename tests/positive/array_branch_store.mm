atom main() -> i64
requires: true;
ensures: result == 50;
body: {
    let a = [1, 2, 3];
    if a[0] == 1 {
        a[1] = 50;
    } else {
        a[1] = 60;
    }
    a[1]
}
