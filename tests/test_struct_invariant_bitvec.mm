// A bitwise struct invariant selects BV(64) semantics for every atom that
// assumes it (parameter), checks it (literal) or imposes it (result).
struct Flags {
    mask: i64,
    bits: i64,
    invariant: (self.bits & self.mask) == self.bits
}

atom flags_new(mask: i64) -> Flags
requires: true;
ensures: result.mask == mask;
body: Flags { mask: mask, bits: 0 };

atom flags_set(f: Flags, extra: i64) -> Flags
requires: true;
ensures: result.mask == f.mask;
body: Flags { mask: f.mask, bits: f.bits | (extra & f.mask) };

atom flags_identity(f: Flags) -> Flags
requires: true;
ensures: result.bits == f.bits;
body: f;

atom flags_bits(f: Flags) -> i64
requires: true;
ensures: (result & f.mask) == result;
body: f.bits;
