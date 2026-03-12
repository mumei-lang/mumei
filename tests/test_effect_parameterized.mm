// =============================================================
// Test: Parameterized effect — should pass verification
// =============================================================
// Effects with parameters and path constraints.

effect FileRead;
effect FileWrite;

atom read_tmp_file(x: i64) -> i64
  effects: [FileRead];
  requires: x >= 0;
  ensures: result >= 0;
  body: {
    perform FileRead.read(x);
    x
  }

atom write_and_read(x: i64) -> i64
  effects: [FileRead, FileWrite];
  requires: x >= 0;
  ensures: result >= 0;
  body: {
    perform FileWrite.write(x);
    perform FileRead.read(x);
    x
  }
