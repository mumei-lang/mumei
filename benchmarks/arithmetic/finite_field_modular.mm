// Success case: modular (finite-field) arithmetic obligations that Z3 answers
// `unknown` (nonlinear + modular), so every atom here is routed to the
// mumei-lean bridge and contributes a measured `lean_solver_time_s`.
// `ff_mul_assoc_bench` and `ff_mul_add_distrib_bench` are not closed by their
// bridge lemma template and are discharged by the automatic tactic search
// (mumei-lean docs/LEAN_TRANSLATOR_SPEC.md §12).
// expected: PASS

atom ff_zero_is_zero_bench(p: i64)
requires: p > 0;
ensures: ff_eq(result, 0, p);
body: { ff_zero(p) };

atom ff_mul_commutes_bench(a: i64, b: i64, p: i64)
requires: p > 0;
ensures: ff_eq(result, ff_mul(b, a, p), p);
body: { ff_mul(a, b, p) };

atom ff_mul_assoc_bench(a: i64, b: i64, c: i64, p: i64)
requires: p > 0;
ensures: ff_eq(result, ff_mul(a, ff_mul(b, c, p), p), p);
body: { ff_mul(ff_mul(a, b, p), c, p) };

atom ff_mul_add_distrib_bench(a: i64, b: i64, c: i64, p: i64)
requires: p > 0;
ensures: ff_eq(result, ff_add(ff_mul(a, b, p), ff_mul(a, c, p), p), p);
body: { ff_mul(a, ff_add(b, c, p), p) };
