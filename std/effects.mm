// =============================================================
// std/effects.mm — Mumei Built-in Effect Definitions
// =============================================================
// Effect annotations declare what side effects an atom may perform.
// The compiler uses Z3 to verify that no undeclared effects occur.
//
// Usage:
//   atom write_log(msg: i64)
//       effects: [Log, FileWrite];
//       requires: msg >= 0;
//       ensures: result >= 0;
//       body: {
//           perform FileWrite.write(msg);
//           msg
//       };
//
// Pure atoms (no effects: field) cannot perform any effects.

// --- Basic Effects ---
effect FileRead;
effect FileWrite;
effect Network;
effect Log;
effect Console;

// --- Composite Effects ---
// IO includes file I/O and console access
effect IO includes: [FileRead, FileWrite, Console];

// FullAccess includes all effects
effect FullAccess includes: [IO, Network, Log];
