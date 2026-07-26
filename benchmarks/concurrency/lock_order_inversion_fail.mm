// Counterexample case: locks are declared (and acquired) from high to low
// priority, which admits a wait-for cycle. The resource hierarchy check must
// reject this atom.
// expected: FAIL

resource ledger_lock priority: 1 mode: exclusive;
resource journal_lock priority: 2 mode: exclusive;

atom inverted_lock_order(x: i64)
resources: [journal_lock, ledger_lock];
requires: x >= 0;
ensures: result == x;
body: {
    acquire journal_lock {
        acquire ledger_lock {
            x
        }
    }
};
