// Deadlock freedom by resource hierarchy: nested acquisitions follow strictly
// increasing resource priority.
// expected: PASS

resource ledger_lock priority: 1 mode: exclusive;
resource journal_lock priority: 2 mode: exclusive;
resource audit_lock priority: 3 mode: exclusive;

atom two_level_acquire(x: i64)
resources: [ledger_lock, journal_lock];
requires: x >= 0;
ensures: result == x;
body: {
    acquire ledger_lock {
        acquire journal_lock {
            x
        }
    }
};

atom three_level_acquire(x: i64)
resources: [ledger_lock, journal_lock, audit_lock];
requires: x >= 0;
ensures: result == x;
body: {
    acquire ledger_lock {
        acquire journal_lock {
            acquire audit_lock {
                x
            }
        }
    }
};

atom single_resource_acquire(x: i64)
resources: [journal_lock];
requires: x >= 0;
ensures: result == x;
body: {
    acquire journal_lock {
        x
    }
};
