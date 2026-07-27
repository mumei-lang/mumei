// `task_group:any` winner selection: the group result must satisfy the
// postcondition regardless of which racing task wins.
// expected: PASS

atom race_two_replicas(a: i64, b: i64)
requires: a >= 0 && b >= 0;
ensures: result >= 0;
body: {
    task_group:any {
        task { a };
        task { b }
    }
};

atom race_with_bounded_replicas(a: i64, b: i64, c: i64)
requires: a >= 1 && a <= 100 && b >= 1 && b <= 100 && c >= 1 && c <= 100;
ensures: result >= 1 && result <= 100;
body: {
    task_group:any {
        task { a };
        task { b };
        task { c }
    }
};

atom race_with_constant_fallback(a: i64)
requires: a >= 0 && a <= 10;
ensures: result >= 0 && result <= 10;
body: {
    task_group:any {
        task { a };
        task { 0 }
    }
};
