// Negative fixture: unknown task_group join semantics must be a clean
// syntax error, not a panic.
atom bad_join_semantics()
    requires: true;
    ensures: result >= 0;
    body: {
        task_group: bogus {
            task a { 1 }
        };
        0
    };
