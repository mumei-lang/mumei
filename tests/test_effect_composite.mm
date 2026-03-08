// =============================================================
// Test: Composite effect — should pass verification
// =============================================================
// IO includes FileRead and FileWrite.
// Atom with effects: [IO] can use both FileRead and FileWrite.

effect FileRead;
effect FileWrite;
effect Console;
effect IO includes: [FileRead, FileWrite, Console];

atom io_operation(x: i64)
effects: [IO];
requires: x >= 0;
ensures: result >= 0;
body: {
    perform FileRead.read(x);
    perform FileWrite.write(x);
    x
};
