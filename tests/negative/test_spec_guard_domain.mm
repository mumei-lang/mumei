atom g(n: u32) -> i64
    requires: true;
    ensures: result == 1;
    body: 1;

atom f(c: bool, n: i64) -> i64
    requires: c => (n >= 0 && n <= 4294967295 && g(n) > 0);
    ensures: c || n >= 0;
    body: 0;

enum Choice {
    Call,
    Skip
}

atom f_match(choice: Choice, n: i64) -> i64
    requires: match choice {
        Choice::Call if n >= 0 && n <= 4294967295 && g(n) > 0 => true,
        Choice::Skip => true,
        _ => false
    };
    ensures: match choice {
        Choice::Skip => n >= 0,
        _ => true
    };
    body: 0;
