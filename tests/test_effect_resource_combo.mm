// =============================================================
// Test: Effects combined with resources — should pass verification
// =============================================================
// Atom uses both resource acquire and effect perform.

effect FileWrite;
resource db;

atom write_with_resource(x: i64)
effects: [FileWrite];
resources: [db];
requires: x >= 0;
ensures: result >= 0;
body: {
    perform FileWrite.write(x);
    x
};
