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

// --- Parameterized File Effects ---
// FileRead(path) constrains which file paths may be read.
// FileWrite(path) constrains which file paths may be written.
effect FileRead(path: Str);
effect FileWrite(path: Str);

// --- Parameterized Network Effects ---
// HTTP method effects with URL parameter for security policy enforcement.
effect HttpGet(url: Str);
effect HttpPost(url: Str);
effect HttpPut(url: Str);
effect HttpDelete(url: Str);

// --- Parameterized Effects with Default Constraints ---
// These constrained variants restrict the parameter domain.
// Used by security policies to enforce safe defaults.
effect FileRead(path: Str) where starts_with(path, "/tmp/");
effect HttpGet(url: Str) where starts_with(url, "https://");

// --- Composite Effects ---
// IO includes file I/O and console access
effect IO includes: [FileRead, FileWrite, Console];

// NetworkIO includes all HTTP method effects
effect NetworkIO includes: [HttpGet, HttpPost, HttpPut, HttpDelete];

// FullAccess includes all effects
effect FullAccess includes: [IO, NetworkIO, Network, Log];
