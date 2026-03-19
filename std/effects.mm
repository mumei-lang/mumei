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

// --- Basic Effects (non-parameterized) ---
effect FileRead;
effect FileWrite;
effect Network;
effect Log;
effect Console;

// --- Parameterized Network Effects ---
// HTTP method effects with URL parameter for security policy enforcement.
effect HttpGet(url: Str);
effect HttpPost(url: Str);
effect HttpPut(url: Str);
effect HttpDelete(url: Str);

// --- Parameterized Effects with Default Constraints ---
// These constrained variants restrict the parameter domain.
// Used by security policies to enforce safe defaults.
// NOTE: FileRead and FileWrite are already defined as non-parameterized above.
// Parameterized overloading is not yet supported by the parser/ModuleEnv.
// When parameterized effect overloading is implemented, uncomment these:
//   effect FileRead(path: Str) where starts_with(path, "/tmp/");
//   effect FileWrite(path: Str);
//   effect HttpGet(url: Str) where starts_with(url, "https://");

// --- Parameterized File Effects with Path Constraints ---
// SafeFileRead/SafeFileWrite restrict paths to /tmp/ and disallow ".." traversal.
// Used by security policies to enforce safe file access at compile time.
effect SafeFileRead(path: Str) where starts_with(path, "/tmp/") && not_contains(path, "..");
effect SafeFileWrite(path: Str) where starts_with(path, "/tmp/") && not_contains(path, "..");

// --- Composite Effects ---
// IO includes file I/O and console access
effect IO includes: [FileRead, FileWrite, Console];

// NetworkIO includes all HTTP method effects
effect NetworkIO includes: [HttpGet, HttpPost, HttpPut, HttpDelete];

// FullAccess includes all effects
effect FullAccess includes: [IO, NetworkIO, Network, Log];

// --- Stateful Effects (Temporal Effect Verification) ---
// Stateful effects define states and transitions for compile-time
// temporal ordering verification (Phase 1i).
// Example: A File effect with Open/Closed states:
//
//   effect File
//       states: [Closed, Open];
//       initial: Closed;
//       transition open: Closed -> Open;
//       transition write: Open -> Open;
//       transition read: Open -> Open;
//       transition close: Open -> Closed;
//
// The compiler verifies that operations occur in valid states:
//   perform File.open(x);   // OK: Closed -> Open
//   perform File.write(x);  // OK: Open -> Open
//   perform File.close(x);  // OK: Open -> Closed
//   perform File.write(x);  // ERROR: InvalidPreState (expected Open, got Closed)
