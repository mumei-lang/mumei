// Success case: additively homomorphic commitments over a prime field. Z3
// answers `unknown` for the modular/nonlinear obligations, so the whole file is
// discharged through the mumei-lean bridge; `commitment_scalar_distrib_bench`
// needs the automatic tactic search (mumei-lean docs/LEAN_TRANSLATOR_SPEC.md §12).
// expected: PASS

atom commitment_homomorphic_add_bench(a: i64, b: i64, r: i64, s: i64, p: i64)
requires: p > 0;
ensures: ff_eq(result, ff_add(ff_mul(b, s, p), ff_mul(a, r, p), p), p);
body: { ff_add(ff_mul(a, r, p), ff_mul(b, s, p), p) };

atom commitment_scalar_distrib_bench(a: i64, r: i64, s: i64, p: i64)
requires: p > 0;
ensures: ff_eq(result, ff_add(ff_mul(a, r, p), ff_mul(a, s, p), p), p);
body: { ff_mul(a, ff_add(r, s, p), p) };
