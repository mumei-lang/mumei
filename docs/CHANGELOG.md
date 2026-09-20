### 2026-09-20: Explicit-type-argument calls `name<T, …>(args)`

- `f<i64>(x)` / `apply<i64, Network>(42, atom_ref(net_fn))` previously
  misparsed as comparisons — `apply < i64` then `>` — silently
  producing nonsense expressions or vacuous passes.
  `tests/effect_polymorphism_mixed.mm` carried such a misparse before
  its call was worked around; it is restored to the real
  explicit-type form.
- The parser now speculatively matches `ident` `<` type-ref-list `>`
  `(` and, only on that exact shape, produces a `Call` whose callee
  name carries the instantiation (`apply<i64, Network>`) — matching
  the monomorphizer's `parse_type_ref(call_name)` collection and the
  atom registry's `display_name()` keys, so lookup, HIR lowering, and
  MIR unbound-name checks work unchanged. Every other shape (`a < b`,
  `a < b > c`, `a < f<c >> d`, `f < i64 > x` with no `(` after `>`)
  rewinds and stays a comparison; `>>` splits are rolled back via a
  token-stream snapshot.
- The monomorphizer also scans `requires`/`ensures`/`invariant`/`forall`
  clause expressions for instantiated call names, so
  `ensures: result == id<i64>(3)` resolves instead of skipping the
  clause as unsupported.
- Unknown callees or unsatisfied `where` bounds stay fail-closed:
  `mystery<i64>(x)` reports `Unknown function: mystery<i64>`; a call
  site violating the monomorphized callee's `requires` fails as a real
  precondition error.
- Tests: parser coverage in `parser/mod.rs` (`test_parse_explicit_type_arg_call`,
  spacing/nesting, comparison regressions, while-cond), CLI harness
  `tests/test_generic_call.rs` over `tests/test_generic_call.mm` /
  `tests/test_generic_call_negative.mm` and the effect-polymorphism
  fixtures.

### 2026-09-20: clause `match` on struct fields resolves the field's type — and must be exhaustive

- `requires:`/`ensures:` clauses lower with `solver_opt = None` so arm-body
  side effects never leak onto the ambient solver — but that also skipped
  the exhaustiveness check and the scrutinee's enum-domain assert. A
  non-exhaustive clause match then fell through the `ite` fold to the
  **last arm's body on every uncovered input**: `requires: match h.r {
  Shape::Point => true }` silently lowered to `true`, a vacuous-verify
  soundness hole.
- `Expr::Match` now runs domain injection and the exhaustiveness check on
  a scratch solver (seeded with `vc.clause_context`, the already-lowered
  clauses of the same spec) whenever no ambient solver is present. A
  non-exhaustive clause match is a `spec_lowering_failed` error;
  exhaustive matches lower exactly as before.
- The `decl_hint` that disambiguates colliding variant names now covers
  `FieldAccess` chains, calls, and `result`, not just bare variables: a
  new `declared_type_of_expr` walks `h.r` / `h.a.b` / `f(x).r` /
  `result.r` through `StructDef.fields` to the field's declared type
  name (`param h: H → H → field r → Shape`). This complements the
  const-name walk in `target_param_enum_name`, which dead-ends on
  scrutinees with no flattened `h_r` const (e.g. `result.r`).
- `infer_expr_enum_name` uses the same walk, so `let s = h.r` records
  `s: Shape` and a later `match s` resolves like `match h.r`.
- Non-enum fields (`match h.n` on `n: i64`) behave exactly like a
  body-level i64 match: literal patterns + wildcard work, and a
  non-exhaustive literal-only match fails closed.
- Tests: `tests/test_clause_match_struct_field.mm` (5 atoms),
  `tests/test_clause_match_struct_field_negative.mm` (4 atoms).

### 2026-09-20: `len` resolves strings and structural array values; scalar args are type errors

- Previously every non-`Variable` `len(e)` argument collapsed onto one
  shared uninterpreted `len_arr` symbol, and `len(x)` on an `i64`
  silently bound a fresh unconstrained constant — so `len("abc") == 3`
  could fail while `len(5)` was vacuously admissible.
- `Str` arguments now map to Z3 `str.len` (`Z3_mk_seq_length`), so
  `len("abc") == 3`, `len(s)` on `Str` params, and `len("ab" + "cde")
  == 5` all reason about real string length.
- Non-variable array arguments (literals, `if`/`match` values, call
  results) resolve through `tail_len_expr`: literals get their concrete
  length, branch values get the mirrored `ite` length merge. Only an
  argument that actually produces a tracked array may fall back to a
  fresh symbol — scalars and untracked variables are a clean type error
  (`len() expects an array or string argument; \`x\` is neither`).
- LLVM codegen keeps pace: `len` on `Str` values emits `strlen` (locals
  and `s + t` results alike), `len` on string/array literals
  constant-folds, and `len` on `if`/`match`/call array values extracts
  the fat pointer's `len` field. The old fallthrough that emitted
  `const 0` for anything it didn't understand is gone — the verify-only
  `len` would otherwise have compiled to wrong code.

 9a349ff (verify: len() resolves Str via str.len and array values structurally; scalars are type errors)
### 2026-09-20: `let`-bound lambdas resolve at indirect call sites

- `let f = |a| a + 1` already bound the lambda, but `f(3)` /
  `call(f, 3)` fell through `Expr::Call`/`Expr::CallRef` to
  `Unknown function: f` — so an `ensures` on the call result was
  unprovable, and a *wrong* `ensures` surfaced as "Counterexample
  unvalidated" (spurious) instead of a genuine counterexample.
- `VCtx` gains `local_lambdas` (`RefCell<HashMap<String, Rc<LocalLambda>>>`,
  next to `local_enum_types`/`local_array_elem_types`), populated by
  `let`/`assign` and cleared by `havoc_vars`/non-lambda rebinds.
  `LocalLambda::Closure` stores the `Expr::Lambda` plus the lambda
  scope captured at binding time, so free lambda names inside the body
  resolve lexically — `let g = …; let f = |a| g(a); let g = …; f(x)`
  keeps the `g` live when `f` was defined.
- `apply_local_lambda` evaluates args in the caller env (call-by-value)
  and runs the body with `param <- arg` bindings plus the usual
  struct-field/array-slot wiring — `f(3)` is the let-in
  `let a = 3 in a + 1`. Higher-order calls work too: `twice(inc, 4)`
  re-binds param `f` to `inc`'s closure, and `call(|a| a * 5, 2)`
  applies an inline literal. A lambda param called while its own body
  is checked at bind time resolves through `LocalLambda::Opaque` to a
  fresh symbolic int (the check's value is discarded; the real callee
  is supplied at each call site).
- Scoping mirrors `local_enum_types`: if/else merges keep a name when
  both branches leave the *same* binding (`Rc::ptr_eq` — aliases
  share the allocation, so `f = g` on both sides survives, while
  textually identical but distinct lambdas drop); one-sided branch
  bindings leak like `merge_branch_envs`; while/match arms snapshot and
  restore the map. Divergent branch bindings, `f = 0` rebinds, arity
  mismatches, and recursive `|a| f(a)` all fail closed.
- `spurious_detection` replays `EvalValue::Lambda` closures
  (params bound in the captured `EvalEnv`), and
  `collect_expr_symbols`/`collect_stmt_symbols` stop flagging
  lambda-bound names as `uninterpreted_function` — `f(3)` where the
  postcondition is wrong now reports a *validated* counterexample.
- Tests: `tests/test_lambda_indirect_call.mm` (12 atoms: direct,
  multi-arg, capture, shadowed params, `call(f, x)`, alias, inline
  literal, higher-order, if/while/match scoping) and
  `tests/test_lambda_indirect_call_negative.mm` (wrong postcondition →
  validated counterexample; arity, rebinding, recursion, divergent
  branches → fail closed), run by `tests/test_lambda_indirect_call.rs`;
  `tests/test_lambda_basic.mm` also verifies now. MIR/codegen for
  indirect calls is unchanged — a `f(3)` local is still lowered as a
  direct `Rvalue::Call`, so `mumei build`/`run` on lambda calls is out
  of scope.


### 2026-09-20: `match`-arm array literals merge their lengths

- `let a = match e { A => [1, 2], B => [3, 4, 5] }` already merged the
  array contents (`ite(c_A, arr_A, ite(c_B, arr_B, …))`), but `len_a`
  stayed an unconstrained symbol — `a[1]` failed the bounds check even
  though it is in bounds on every arm.
- `wire_array_slots` now walks the result's `ite` spine and mirrors the
  same conditions on lengths: `len_a = ite(c_A, len_A, ite(c_B, len_B,
  …))`. Arm tails contribute a concrete length for literals, the tracked
  `len_<src>` for variables, and — since arm-local `let`s never reach
  the merged env — the rhs of `A => { let t = […]; t }` is resolved via
  `stmt_let_rhs`. Anything unresolvable gets a fresh `#m<idx>` symbol.
- The merge is recursive: a nested `if`/`match` in an arm (or `if`
  branch) tail mirrors the value node's own `ite` children, so
  `match e { A => if c { [1,2] } else { [9,9,9] }, B => [3,4] }` yields
  `len_a = ite(c_A, ite(c, 2, 3), 2)` — closing the conservative
  nested-`if` gap left by the previous entry too. Depth is bounded
  (`TAIL_LEN_DEPTH`) so `let x = x`-style cycles bail out fresh.
- Bounds stay per-arm precise: `a[2]` fails closed on any path whose
  arm/branch has 2 elements.
- Tests: `match_arm_lit_len` / `match_arm_local_let` /
  `match_arm_var_len` / `match_arm_three_way` /
  `match_arm_nested_if` / `match_arm_nested_match` /
  `if_branch_nested_if` in `tests/test_array_literal.mm`,
  `match_arm_lit_oob` / `match_arm_nested_if_oob` in
  `tests/test_array_literal_negative.mm`.

### 2026-09-20: `if`-branch array literals merge their lengths

- `let a = if c { [1, 2] } else { [3, 4, 5] }` already merged the array
  contents (`ite(c, arr1, arr2)`), but `len_a` stayed an unconstrained
  symbol — `a[1]` failed the bounds check even though it is in bounds on
  both branches, and `len(a)` proved nothing.
- `wire_array_slots` now reuses the ite condition (child 0 of the merged
  array) to set `len_a = ite(c, len_t, len_e)`: literal tails contribute
  their concrete length, a `var` tail contributes the tracked `len_<src>`
  (including `let`-bound block tails like `if c { let t = [1,2]; t }
  else { … }`), and anything else falls back to a fresh per-side symbol
  (`len_<name>#then` / `#else`).
- Bounds are per-branch precise: `a[2]` still fails closed on the short
  branch. Nested `if` inside a branch tail keeps the conservative fresh
  length (outer ite merges what it can).
- Tests: `if_branch_lit_len_read` / `if_branch_lit_len_pred` /
  `if_branch_block_tail` in `tests/test_array_literal.mm`,
  `if_branch_lit_oob` in `tests/test_array_literal_negative.mm`.

### 2026-09-20: rebinding an array var to a scalar clears its tracked array state

- `let a = [1, 2]; a = 5; a[0]` **verified** (`a[0] == 1`) even though
  `a` is the integer `5` — `wire_array_slots` returned early on the
  non-array value without removing `__z3_arr_a`/`len_a`, so array
  indexing kept reading the pre-rebind `[1, 2]` chain.
- Simply removing the slots is not enough: `z3_dynamic_array` falls back
  to `Array::new_const(ctx, name)` when `__z3_arr_<name>` is absent, and
  Z3 interns consts by name — for a `[T]` **parameter** that re-derives
  the original param const, resurrecting requires-side assertions
  (`arr = 5; arr[0]` still proved `arr[0] == 4`). The non-array arm now
  installs a `#`-suffixed fresh array + length (`{name}#rebound`,
  `len_{name}#rebound` — `#` can never appear in a source identifier, so
  no interning collision): post-rebind `a[i]`/`a[i] = v`/`len(a)` read an
  unrelated array and fail closed (`Potential Out-of-Bounds`).
- Rebinding to a scalar keeps the value usable (`a` → `5`), rebinding
  back to an array re-wires the slots, scalar → array rebinding works,
  and `let b = a` aliases stay isolated (`a = 5` does not poison `b`).
- Tests: `tests/test_array_literal_negative.mm` gained
  `scalar_rebind_stale_read` / `scalar_rebind_stale_store`;
  `tests/test_array_literal.mm` gained `rebind_scalar_tail` /
  `rebind_back_to_array` / `scalar_to_array`.

### 2026-09-20: string literals re-escape on body re-lex (`\"`, `\\`, `\n`, `\t`)

- The lexer stores DECODED string content (`\n` → real newline, `\"` →
  real quote), but three re-serialization sites wrote it back raw:
  `append_token` (`collect_brace_body` — the atom-body round-trip),
  `Token::StringLit`'s `Display`, and `legacy_tokenize`'s source
  reconstruction. Any escaped char corrupted the body: `"a\"b"` closed
  the literal early and left `b"…` to parse as bare identifiers
  (`unresolved variable(s) in body`), and `"a\\nb"` (backslash-n, two
  chars) silently re-lexed as `"a<newline>b"` — a different value, so
  e.g. comparing the two literals produced a wrong equality result with
  no error.
- New `escape_string_content` re-encodes `\`, `"`, newline and tab;
  all three sites use it. Backslash-n and newline literals stay
  distinct; embedded quotes round-trip.
- The same raw-write pattern existed at the `Expr::StringLit` level in
  `expr_to_source_string` (call_graph.rs — feeds requires-substitution
  text that is re-lexed), `trace_evaluated_expression`
  (dataflow_inference.rs) and `expr_to_source` (loop_detector.rs, which
  used Rust `{:?}` — mostly-correct but diverges on chars the Mumei
  lexer doesn't decode); all three now use the same helper.
- Tests: `tests/test_string_literal_escape.mm` (quote / tab /
  distinct-escape verified) + `tests/negative/string_literal_escape_collapse.mm`
  (`"a\\nb" == "a\nb"` correctly fails) + `tests/test_string_literal_escape.rs`.

### 2026-09-20: quantifier binders in body `let` are scoped (unbound-check false positive)

- `let ok = forall(i, 0, n, i <= n)` inside an atom body was rejected by
  the Phase 1h unbound check (`unresolved variable(s) in body: i`):
  `forall`/`exists`/`sum`/`all`/`any`/`prod`/`count` lex as ordinary
  `Expr::Call`s and MIR lowering visited every argument, so the binder
  name was looked up as a free variable (pre-existing since the unbound
  check landed; clause-side clauses already exempted the binder via
  `is_binder_call`).
- `lower_expr`'s `Call` arm now allocates a dedicated i64 local for the
  binder and registers it in `var_map` **only while lowering the trailing
  body argument** — `alloc_local` auto-registers named locals, so the
  prior binding is restored immediately after allocation and again after
  the body argument lowers. The binder shadows an outer binding of the
  same name inside the quantifier body, stays OUT of scope in the bound
  arguments (`forall(i, i, 5, …)` → `unresolved variable(s) in body: i`)
  and does not leak past the call (`…; i` after the `let` → same error).
  Any OTHER name inside the body argument still fails closed
  (`forall(i, 0, 5, j > 0)` → `unresolved variable(s) in body: j`).
- `forall`/`exists` in a body remain verification-only constructs:
  codegen rejects them with a clean `Unknown function forall` error
  (fail-closed, no silent miscompile).
- Tests: `tests/test_forall_body_binder.mm` (forall/exists in `let`, true
  and false predicates) + `tests/negative/forall_body_binder_unbound.mm`
  + `tests/test_forall_body_binder.rs`.

### 2026-09-20: parser body-panics become clean syntax errors; `if`-guard propagated to call-site `requires`

- `verify`/`build` on `tests/test_libc.mm` and `tests/test_libc_contracts.mm`
  panicked (`internal error (panic)`) at `parse_prefix` — the files used
  `if cond then … else …`, and the parser's missing-`else` path was a hard
  `panic!`. `then` is not part of the grammar; the tests now use canonical
  `if c { … } else { … }`.
- All three body-level `panic!`s in `mumei-core/src/parser/expr.rs` are now
  recorded `syntax_failure`s with a poisoned recovery: a missing `else`
  yields an `__mumei_missing_else_branch` unbound marker (MIR lowering
  still rejects it if a caller ignores the diagnostics), a `while` without
  `invariant` binds `__mumei_missing_invariant`, and an unknown
  `task_group:` join qualifier records the failure and defaults to `all`.
- New `parse_body_expr_checked` surfaces those diagnostics:
  `Monomorphizer::collect` accumulates them in `body_parse_failures` and
  the shared load pipeline returns a clean
  `Syntax error(s) in atom bodies of '<file>': atom '<name>': <detail>`
  for verify/check/build — non-zero exit, no panic.
- Call-site `requires` (and trait `param_constraints`) checks now assert the
  enclosing `if`/`else` branch guards from `path_cond_stack` before
  querying the solver — the same pattern the shift-range and
  div-by-zero checks already used. `libc::safe_free(ptr)` inside
  `if ptr >= 0` now verifies instead of failing with a spurious
  `ptr = -1` counterexample.
- Tests: `tests/test_if_else_required.rs` +
  `test_if_guard_requires.mm`, `test_if_missing_else_negative.mm`,
  `test_while_missing_invariant_negative.mm`,
  `test_task_group_bad_join_negative.mm`.
### 2026-09-20: array literals `let a = [e0, e1, …]` — parse, verify, codegen

- **`Expr::ArrayLit`** — a `[` at expression position (prefix) now parses an
  element list instead of falling through to the catch-all that silently
  produced `Expr::Number(0)`. `[]` alone cannot infer an element type, so it
  records a `syntax_failure` and emits a `__mumei_empty_array_literal`
  marker (the checked body-parse path turns it into a clean error, and the
  marker is unbound downstream if a caller ignores the diagnostics).
- **Verify** — the literal lowers to a fresh `Int -> Elem` store chain. A
  `let`/`assign` binding wires the name-keyed slots
  (`__z3_arr_<var>`, `len_<var>` = concrete `n`, `local_array_elem_types`)
  via the new shared `wire_array_slots` helper — the same helper now also
  wires **call arguments** (`head([7,8,9])` satisfies `requires: len(arr)>=1`)
  and the **`result` tail** (`atom … -> [i64] { [1,2,3] }` answers
  `ensures: forall(i,0,3, result[i]==i+1)`). Alias `let b = a` copies `len_a`.
  For `var` sources the wired array is the tracked `__z3_arr_<src>` chain —
  `env[src]` holds only the base const (stores never rewrite it), so
  `arr[0] = 9; let a = arr` now sees the post-store array in `a`/`result`/callee.
  Element sort is the *widest* across elements (Float > Real > Int; bool
  literals must be uniformly Bool) — a first-element probe mis-sorted
  `[1.0, 2.5, 4.0]` because Z3 numerals for whole floats report `Int`.
  Nested `[…[…]…]` and `["x","y"]` fail closed (element sorts Array/Seq
  unsupported — same frontier as `[Str]` array params).
- **Callee array stores now reach the caller model (unsoundness fix)** —
  a `[T]` argument hands the callee the `{len, ptr}` fat pointer, so
  `arr[i] = v` inside a callee lands in the caller-visible buffer; the
  verifier previously kept the caller's pre-call store chain and could
  "verify" stale claims like `a[0] == 1` after `mutate(a)`. Call sites now
  havoc the tracked chain (and every `let b = a` alias sharing its root) of
  args whose callee parameter may be stored through — detected by
  `atom_stores_to_array`, a depth-capped transitive scan over
  `Stmt::ArrayStore`/call-argument positions (unknown or deeply-nested
  callees assume mutation, fail closed). The same havoc is applied to the
  callee param's slot in `call_env` before `ensures` evaluation so
  `arr[i]` in a mutating callee's postcondition means post-call contents.
  Pure callees keep full precision — `head(a); a[0]` still knows `a`.
- **Codegen array call arguments emit the fat pointer** — a `[T]` argument
  previously emitted only the `len` i64 while the callee's signature took
  `{i64, ptr}`, producing invalid IR (`call i64 @bump(i64 %p_len)` against
  `declare i64 @bump({i64, ptr})`). Array args now build the `{i64, ptr}`
  aggregate via `insertvalue` from `array_ptrs` (or materialise a literal
  inline) — verified parseable by `llvm-as-17`.
- **While-loop havoc resets tracked array slots (unsoundness fix)** —
  `havoc_vars` rebound `env[name]` for loop-modified vars but left
  `__z3_arr_<name>`/`len_<name>` on the pre-loop store chain, so
  `let a = [1,2]; while …{ a = [3,4,5] }; a[0]` could "verify" the stale
  claim `result == 1`. Havoc now swaps in a fresh array const of the same
  element sort plus a fresh `len_` symbol; post-loop proofs need an
  `invariant:` pinning `len(a)` and/or elements.
- **Codegen** — `emit_array_literal` materialises the elements into an
  `alloca`'d `[n x elem]` with element-typed GEP stores (int→f64 widens via
  `sitofp`), returning the `(len, elem_ty, data_ptr)` fat pointer that
  `array_ptrs` already tracks. `let a = […]` registers `a` exactly like an
  `[T]` parameter; `let a = arr` now aliases the fat pointer too (was
  "Array 'a' not found as fat pointer parameter"). A `-> [T]` tail of
  `ArrayLit`/`Variable` rebuilds the `{i64, ptr}` return aggregate.
- **Pre-existing fix surfaced by this work** — `Token::FloatLit` Display
  printed `1.0` as `"1"`, and `collect_brace_body` re-lexes the atom body
  from token text, so every whole-valued float literal in a *body*
  (`let x = 1.0`) silently became `Expr::Number(1)`. Display now emits
  `{n:?}` so whole floats round-trip. (Clause text like `result == 1.0`
  was already collected losslessly.)
- Tests: `tests/test_array_literal.mm` (10 atoms: read/store/alias/f64/bool/
  reassign/tail-forall/call-arg/symbolic/…) + `tests/test_array_literal.rs`
  (+ `test_array_literal_negative.mm` — `a[3]` on a len-3 literal rejected).

- Backlog edges noted: rebinding an array name to a scalar (`a = 5`) leaves
  `__z3_arr_a`/`len_a` stale (same class as the `#603` alias edge — MIR has
  no reassign typecheck yet); `StringLit` re-quoting in `collect_brace_body`
  loses escapes (`"a\nb"`); struct-field scrutinees in `requires`/`ensures`
  clauses still lack declared-type resolution.

### 2026-09-20: counterexample report no longer flags translated builtins as uninterpreted

- `collect_expr_symbols` (spurious-CE detection) treated every `Expr::Call`
  to a name absent from `module_env.atoms` as an `uninterpreted_function`
  dependency — including translator-handled builtins (`len`, `forall`,
  `exists`, `matches`/`match_regex`/`re_match`, `sqrt`, `cast_to_int`, string
  predicates). A clause like `requires: len(arr) == 3` made genuine
  counterexamples get labeled "spurious — escalate to Lean" even though the
  solver had the real model.
- `is_translated_builtin` exempts those names, so genuine failures report as
  counterexamples again. Regression: `tests/negative/builtin_not_spurious.mm`
  + `tests/test_builtin_cex_label.rs`.

### 2026-09-20: `let a = arr` aliases share the tracked array (len + store history)

- `Stmt::Let`/`Stmt::Assign` with a bare `Variable` value now propagate the
  source's `__z3_arr_<name>` backing const and `len_<name>` symbol to the
  alias: `let a = arr` then `a[i]` reads/stores share `arr`'s constraint
  state. Previously the alias lowered to a fresh unconstrained array, so
  `a[0]` failed bounds checks even when `forall(i,0,n,arr[i]…)` pinned
  `len_arr >= n`.
- Chained aliases (`let b = a` where `a` aliases `arr`) propagate
  transitively. Arrays are Move types — `arr` is consumed by the alias, so
  post-move reads/writes go through the alias name.
- Tests: `tests/test_alias_probe.mm` (alias read/store/chain) +
  `tests/test_array_alias.rs`.

### 2026-09-20: clause-scope phantom names fail closed; unbound-check exemptions aligned

- `requires` / `ensures` / `invariant` / `forall_constraints` clauses that
  reference undeclared names now fail verification with
  `unresolved name(s) in <clause>: <names>` — the clause counterpart of the
  Phase-1h body check. A phantom name in `requires` made the precondition
  trivially satisfiable (vacuous verify); in `ensures` it produced spurious
  postcondition failures. Quantifier binders (`forall(i, ...)`), clause-local
  `let`s, match-arm bindings, `result` (ensures only), `true`/`false`, and
  module-level type/atom names are recognised as bound.
- Body-check follow-up (`unbound_names` exemptions): names of declared
  module items — enums (`Shape` in `Shape::Point`), structs, atoms
  (`atom_ref(name)`), type aliases, and effects (`FileWrite` in
  `perform FileWrite.write`) — no longer count as unbound variables.
  `consume x`/`ref x` params are bound under their bare identifier so `x`
  resolves in the body (the parser keeps the keyword in `Param.name`).
- Generic type-parameter items (`atom pipe<E>` etc.) are dropped from the
  post-monomorphization item list; their names are now registered in the
  module env anyway so references from other bodies stay resolvable.
- Surfaced phantom-name sites fixed honestly: `std/{compliance,settlement,
  container/verified_vector}.mm` and ten `tests/*.mm` files referenced `arr`
  (or `brr`) in requires/body without declaring it — they now declare
  `arr: [i64]` params (**signature changes**, e.g.
  `verify_all_transactions_compliant(arr, n, limit)`). `negative/*.mm`
  fixtures declare their arrays so they fail on the intended violation.
- `tests/test_untyped_array_access.mm::uses_untyped_array` declares
  `arr: i64` (annotated but non-`[T]`) — the `untyped_array_access`
  diagnostic still fires; fully-undeclared names are now a hard error.
- `tests/effect_polymorphism_basic.mm::main` called `pipe<FileWrite>(..)` —
  generic call-site syntax does not exist (parses as a comparison chain);
  it now calls `writer(42)` directly.
- Tests: `tests/negative/clause_unbound_name.mm` +
  `tests/test_clause_unbound_name.rs`.

### 2026-09-20: unbound-check follow-up — lambda params bound; bogus generic-call syntax surfaced

- `HirExpr::Lambda` lowering now registers the lambda's parameters as MIR
  locals (with their declared types) before lowering the body and restores
  outer bindings afterwards. Previously `|a| a` left `a` unbound, so the new
  unbound check reported the parameter itself. Lambda bodies that call the
  lambda indirectly (`call(f, x)` on a `let`-bound lambda) still fail closed
  with a spurious-counterexample report — indirect lambda calls are
  uninterpreted, a pre-existing verifier limit.
- `tests/effect_polymorphism_mixed.mm::main` called `apply<i64, Network>(..)`
  — explicit type-argument call syntax does not exist; it mis-parsed as the
  comparison `apply < i64` and verified vacuously through the phantom-local
  path. The atom now calls `net_fn(42)` so the effect-polymorphism path is
  exercised for real. Generic-atom call sites remain unsupported (both
  explicit `<T, E>` args and inference-only calls lower to uninterpreted
  calls).

### 2026-09-20: MIR unbound-variable check fails closed (phantom `Local(0)` fix)

- `LowerCtx::lookup_var` silently aliased every unbound name to `Local(0)`
  (the atom's first local): a mis-parsed or mistyped name — e.g. a bare
  `task` used outside a `task`/`task_group` tail — bound to the first
  parameter and move analysis then reported phantom `use of moved value`
  / `ConflictingMerge` violations on an innocent local.
- Unbound names now get a dedicated `__unbound:<name>` local recorded in
  `MirBody::unbound_names`; Phase 1h fails verification with
  `unresolved variable(s) in body: <names>` before move analysis runs.
  `result` (the implicit named return) and bare nullary enum variants
  (`Red`) are exempt — they are legal unbound idents.
- Surfaced pre-existing phantom bindings in `std/list.mm`: six fold/sort
  atoms referenced `arr[i]` without declaring `arr` (vacuous-verified
  against an unconstrained Z3 array). They now declare `arr: [i64]` —
  a **signature change**: `fold_sum(n)` is `fold_sum(arr, n)`, etc.
- Clause-scope references (e.g. `forall(i, 0, n, arr[i] >= 0)` in
  `requires` without a body/param `arr`) still verify against a fresh
  unconstrained array — the same pre-existing leniency, unchanged.
- Tests: `tests/test_mir_unbound_variable{,_negative}.{mm,rs}`.

### 2026-09-20: `[f64]`/`[bool]`/`[str]` arrays lower to fat pointers in codegen

- `resolve_param_type`/`resolve_return_type` only recognized
  `[i64]` as the `{ i64 len, T* data }` fat pointer; `[f64]`/`[bool]`/
  `[str]` params fell through to `i64`, so `arr[i]` failed codegen with
  `Array 'arr' not found as fat pointer parameter` even though the same
  code verified. Any `LoweredType::Array` now lowers to the fat pointer.
- `array_ptrs` is now `(len, elem_ty, data)` — the declared element type
  (`array_elem_llvm_type`: `f64`→double, `str`→ptr, bool/other→i64 —
  bool keeps the i64 convention) drives the GEP/load/store in
  `ArrayAccess` and `ArrayStore` instead of a hardcoded `i64`. Storing an
  integer into a `[f64]` emits `sitofp` (`arr[0] = 42` → `store double
  4.2e+01`); other mismatches go through `bitpreserve_cast` or a clean
  codegen error. Task bodies capture arrays with their element type, so
  the inner fat pointer is rebuilt correctly.
- Array returns now rebuild the fat pointer: a tail `arr` emits
  `insertvalue` len+data into `{ i64, ptr }` and `ret`s it. Previously an
  array-typed return emitted `ret i64 %arr_len` under a `{i64, ptr}`
  signature — invalid IR (and on `[i64]` even a silently-wrong len-only
  `i64` return). Non-array tails under an array return type now fail with
  a clean codegen error (e.g. `let a = arr; a` aliases and array
  literals are not yet supported in HIR).
- `test_polymorphic_array.mm`'s `test_i64_array` declared no `arr`
  parameter at all — the phantom name verified vacuously against an
  unconstrained Z3 array — now `arr: [i64]` like its siblings; the file
  builds end-to-end (`mumei build --emit llvm-ir`) for the first time.

### 2026-09-20: lowercase `qual::Variant` patterns parse as qualified paths

- `parse_pattern` folded `::`-separated path segments only for uppercase
  idents, so `mine::Cons(v)` bound `mine` as a variable pattern and
  orphaned `::Cons(v)` into the arm tail — the qualifier was silently
  dropped (and the arm could misbind or mis-verify). `::` folding is now
  case-independent: any `ident::seg` path parses as a qualified variant
  reference — lowercase qualifiers resolve by leaf name through the same
  module-path semantics as before.
- Tests: `test_pattern_lowercase_qual.mm` — lowercase qualifier binds the
  payload; unknown module qualifier resolves by leaf.

### 2026-09-20: match arm bindings stay arm-local (verify)

- `merge_arm_env_into` folded every differing key of the arm env into the
  post-match env, so names introduced by the arm — pattern bindings and
  `let`s — leaked outward. A pattern variable shadowing an outer `let`
  (`let x = 99; match p { P(x, b) => … }`) left `x` bound to the payload
  after the match, and an arm-body `let` shadowing an outer name folded
  back as if it were an assignment.
- The merge now skips the arm-local name set: names collected from the
  arm's pattern (`collect_pattern_bindings`, now `pub(crate)`) plus `let`
  names anywhere in the arm body whose envs reach the arm env (blocks,
  `if` branches, `acquire`/`task`/`task_group` bodies — loop bodies and
  inner match arms run on cloned envs and are excluded deliberately).
  Real `Assign`s to pre-existing variables still fold under the arm
  condition, so `match` bodies can update outer state as before.
- Known edge (pre-existing, unchanged by this fix): an arm that assigns an
  outer name and *then* declares a same-named `let` (`{ x = 7; let x = 9 }`)
  reports the pre-match value for `x` after the match — the outer write is
  dropped instead of surviving as `7`. Before this fix the same program
  leaked the shadow value `9`, so the pattern was already wrong; the fix
  changes which wrong answer it produces. Splitting outer writes from
  shadowed writes for one name needs ordered env tracking — backlog.
- Tests: `test_match_arm_scoping.mm` (pattern shadow, let shadow,
  outer-assign fold), `test_match_arm_scoping_negative.mm` (a write to a
  shadowed name must not reach the post-match env).

### 2026-09-20: fragment classifier resolves match owners via declared enum types

- `detect_logic_fragment_tags` now threads a declared-enum-type
  environment through `stmt_has_inductive_shape` /
  `expr_has_inductive_shape`: parameters seed it and `let`/`assign`
  bindings extend it (`expr_enum_name` resolves `E::V(..)`/`E::V`
  constructors, enum-typed variables, enum-returning atom calls, and
  `if`/`match` whose branches agree). The environment is scoped — blocks
  clone it, arm/branch/loop bodies do not leak their `let`s back out.
- A `match` arm's variant owner now resolves via
  `ModuleEnv::resolve_variant_owner_by_hint(variant_name, hint)` where
  `hint` is the scrutinee's inferred declared type. Previously a bare
  variant name colliding across enums with different tags (e.g. `Ok`
  declared by both `R1` and `R2`) was irresolvable and the atom kept the
  `inductive_data_type` tag even though the finite-ADT datatype path
  verifies it natively — `match r { Ok(v) => …, Err => … }` on `r: R1`
  (or on `let r = R1::Ok(..)`) now stays in the decidable fragment.
  Unresolvable owners still keep the tag (the verifier fails closed).
- `resolve_variant_owner_deterministic` removed — its only caller was the
  classifier, now on the hint-aware `resolve_variant_owner_by_hint`.
- Tests: `test_colliding_variant_uses_declared_param_type`,
  `test_colliding_variant_uses_let_binding_type`.

### 2026-09-20: MIR match pattern bindings + merge-aware move analysis

- **MIR lowering** (`mumei-core/src/mir.rs`): `match` arm patterns now emit
  real locals — `Variable` binds the scrutinee (`Rvalue::Use`), `Variant`
  field patterns bind positional `Rvalue::FieldAccess` projections (nested
  variants recurse through a temporary). Previously pattern-bound names had
  no `var_map` entry, so `lookup_var` fell back to `Local(0)` — the first
  local — making `let q = p; match q { Mk(a,s,f) => a+s+f }` a spurious
  "use of moved value `p`" UseAfterMove. Bindings are scoped per arm
  (`var_map` saved/restored) so a pattern variable shadowing an outer name
  does not leak past the arm or into sibling arms.
- **MIR binding types** (`mumei-core/src/mir.rs`): pattern-bound locals now
  carry the variant field's declared type (resolved through `enum_defs`,
  `Self` → the owning enum), and `infer_hir_ty` resolves a `match` arm's
  tail variable to that same field type. An `i64` payload binding is
  therefore Copy — `let r = match p { P(a,b) => a }; let m = r` no longer
  reports `r` as moved — while `Str` payloads stay Move and still trigger
  `UseAfterMove`/`ConflictingMerge`.
- **Move analysis** (`move_analysis.rs`): a `ConflictingMerge` is now only
  reported when the divergent local is live at the merge block
  (`liveness.live_in`). A local moved on one branch but never used again
  (`let r = …; if c { r } else { 0 }`) is dead on every continuation path —
  the divergent ownership is unobservable — so it is no longer a violation.
  Conflicts on locals used after the merge are unchanged, and the merged
  state still marks them dead so later uses fail as UseAfterMove.
  `analyze_moves` computes liveness internally (no signature change); the
  executor additionally runs `compute_liveness` + `insert_drops` before
  analysis so scope-end drops are explicit.
- **`infer_hir_ty`** gained a `HirExpr::Match` arm (infers the result type
  from arm tail expressions) so `let r = match …` bindings of scalar
  results are Copy rather than conservatively Move.
- Tests: `test_move_analysis_merge_unused_move` (unit),
  `test_mir_match_bindings{,_negative}.mm` + `test_mir_match_bindings.rs`.
  `test_move_analysis_conflicting_merge` updated — its merge block now
  returns the moved local so the conflict is observable.
- Known adjacent gap (separate layer, pre-existing): the *verifier* (Phase
  5, HIR executor) also leaks pattern bindings past the match — `let x = 99;
  match p { P(x,b) => … }; if x > 0 …` sees `x` as the arm payload after the
  match, not the outer `x`. MIR now scopes correctly; the spec-side
  environment needs the same treatment.

### 2026-09-20: prefix `!` (logical not) parses in expressions and spec clauses

- `!e` desugars to `if e { false } else { true }` at parse time — the lexer
  already produced `Token::Bang`, but `parse_prefix` had no arm for it, so
  `!x` silently fell through the catch-all to `Number(0)` and the operand
  token was left dangling. A relational desugar (`e == false`) is not used
  because the comparison-chain normalizer would flatten `!(a < b)` into
  `a < b && b == false`.
- Works in bodies (`if !(b == 0)`), `let` initializers, `requires` /
  `ensures` / `invariant` clauses, and match-arm guards. The operand must
  lower to Bool — `!n` on an `i64` fails closed with "If condition must be
  boolean" (same discipline as `&&`/`||`), and codegen emits the 0/1
  branch identical to `if e { 0 } else { 1 }`.
- Tests: `tests/test_bang_not{,_negative}.mm` + `test_bang_not.rs`.

### 2026-09-19: `match` on `let`-bound enum values resolves the declared type

- **Verify** (`mumei-core`): a `let`/`assign` binding now records the inferred
  declared enum type of its value in `VCtx.local_enum_types` —
  `infer_expr_enum_name` resolves `E::V(..)`/`E::V` constructors, enum-typed
  variables and params, `if`/block tails whose branches agree, `match`
  expressions whose arms agree, and calls to atoms declared to return an
  enum. `match e` after `let e = IntList::Nil` resolves `Cons`/`Nil` to
  `IntList` deterministically instead of failing "Ambiguous enum variant"
  against the prelude `List` (whose `Cons` has a different tag). Rebinding
  to a value with no inferable enum type clears the entry.
- **Codegen** (`mumei-emit-llvm`): `HirStmt::Let` accepts an inferred type
  that resolves to an enum (not only a struct), and `infer_struct_type_name`
  types `if`/`else` expressions whose branches agree — `let e = if c {
  Mine::Cons(1) } else { Mine::Nil }; match e` now emits the match instead
  of erroring ambiguously.
- **Branch scoping**: the inferred-type map is now scoped like the value
  env — `if`/`else` branches run on snapshots and keep only entries both
  branches agree on, and `while`-loop step/termination checks restore the
  map so a loop-body `let` cannot leak a type onto outer bindings.
- **Qualified arm patterns** (`parser` + resolvers): `match e {
  Mine::Yes(v) => .. }` folds `E::V` into the arm's variant name; the
  qualifier pins the owning enum and must agree with the target's declared
  type — `match e { Other::V => .. }` on `e: Mine` fails closed with a
  "belongs to enum" error instead of silently matching `Mine`'s `V`.
  Previously the qualifier was silently dropped (a `Mine::Yes` arm parsed
  as a dead `Mine` arm plus a bare `Yes` arm), and `variant_owners`
  lookups only matched the leaf segment.
- Tests: `test_datatype_enum.mm` gains `let_bound_ctor_match` (recursive
  `IntList` let-bound through `Nil`, matched against colliding prelude
  variants), plus `test_enum_qualified_pattern{,_negative}.mm` covering
  qualified arms on datatype, Int-tag, and let-bound targets.

---

### 2026-09-19: enum parameters bind whole values; `==`/`!=` on enums is deep equality

- **Enum parameters were masquerading as fat-pointer arrays** (`driver.rs`):
  an atom parameter declared with an enum type used to be split as `{ ptr, len }`,
  binding only field 0 (the tag) in `variables`. Three latent bugs followed:
  `match m { Cons(v) => v }` bound `v` to `0` instead of the payload (the
  verifier proved the payload-selector semantics — a silent verify/codegen
  divergence), `a == b` compared the tag alone, and forwarding the parameter
  to a callee passed `i64` where `{ i64, … }` was expected. Parameters now
  check the declared enum too and bind the whole tagged-union struct.
- **Deep equality for same-enum operands** (`expr_emit.rs` BinaryOp): `==`/`!=`
  between two values of the same enum type emit a tag compare AND-ed with a
  per-payload-slot compare (`icmp`/`fcmp oeq`/`mumei_str_eq` for int, float,
  and `Str` slots), `zext`ed to `i64` — matching the verifier's deep datatype
  equality (P10-C). `undef` slots of unit variants are skipped (`and` with
  `undef` would poison the conjunction). Different enum types or struct
  operands still fail with a clean codegen error.
- **`len()` on a non-array binding is now a clean codegen error** instead of a
  silently emitted `i64 0`: `len(m)` on an enum/struct/`Str` value, or
  `len(a)` on a `let`-bound array literal, previously compiled to
  `ret i64 0` — wrong code. Verified before fix that real array parameters
  (fat pointers) keep working.
- Tests: `test_codegen_enum_resolution` gains deep-equality,
  struct-still-errors, payload-binding, and `len`-error cases; SPEC_GUIDE's
  tagged-union section documents the semantics and the fixed parameter bugs.

---

### 2026-09-19: docs-sync — V1-A〜V1-D sections rewritten to implementation reality

- `CROSS_PROJECT_ROADMAP.md`: the "現状のギャップ / 追加すべき機能 / 実装ファイル"
  blocks for V1-A〜V1-D still described the planned surface (`verify-spec`,
  `code_verifier.py`, `verify_spec_soundness`, `check_spec_satisfiability`,
  `verify_spec_code_conformance`) as unimplemented. Rewritten to the actual
  implementation: `audit` / `validate-spec` / `validate-code` /
  `validate-spec-to-code` / `validate-code-to-spec` / `verify-conformance` /
  `verify-traceability` CLIs and the `check_spec_health` /
  `check_spec_contradiction` / `validate_nl_spec` / `validate_code` /
  `validate_spec_to_code` / `validate_code_to_spec` / `verify_conformance` /
  `verify_code_spec_traceability` / `verify_foreign_code` MCP tools. Remaining
  honest gaps: V1-A-2 domain-template completeness warnings and V1-B-3 fix-diff
  auto-generation.

---

# 📝 Changelog

---

### 2026-09-19: enum constructor expressions reach codegen (`E::V(..)` / `E::V`)

- **HIR lowering** (`hir.rs`): qualified constructor calls `E::V(args)` lower
  to `VariantInit` when the qualifier is a known enum declaring the variant —
  a known atom name still wins, and module paths (`mod::f(..)`) or method
  calls (`obj.f(..)`) stay `Call`. The `.`-spelled call form is deliberately
  not converted: verification rejects `E.V(..)` as an unknown function, so
  converting it would let codegen accept a program the verifier refused.
- **Codegen** (`mumei-emit-llvm`): the `VariantInit` emission is extracted to
  `emit_enum_variant_init` (tag `insertvalue` + per-slot `bitpreserve`
  payloads) and shared by two sites — the `VariantInit` arm and the
  `FieldAccess` arm, where an unbound `E::V` / `E.V` unit variant constructs
  the value (a *bound* variable named like the enum keeps field-access
  semantics, matching the verifier's `env.contains_key` shadow precedence;
  `let m = Mine::Nil` also records `m: Mine` in `var_types` so a later
  `match m` resolves the owner). Two fail-closed guards: constructing a
  recursive enum errors cleanly (`enum 'IntList' is recursive`) instead of
  recursing forever in the eager `enum_llvm_type`, and an arity/unknown-
  variant mismatch is an explicit codegen error rather than silent `undef`
  payload (verification rejects both before codegen is reached, so the guard
  is defense-in-depth). Binary operators on aggregate values (`m == Mine::Nil`)
  are a clean codegen error instead of an inkwell `into_int_value` panic;
  `a == b` between enum-typed parameters keeps the pre-existing tag-field
  comparison (payloads are not compared — a documented divergence from the
  verifier's deep datatype equality).
- **Tests**: `test_codegen_enum_resolution.rs` +5 — qualified payload/unit
  ctors emit the owning enum's tag under a prelude `Cons` collision, a
  `let`-bound unit ctor types the owner for `match`, ctor values flow into
  match targets and callee params, enum equality errors cleanly, and a
  recursive ctor fails cleanly.
- **Docs**: `SPEC_GUIDE.md` documents constructor support and its limits
  (recursive enums, positional union payload slots, mixed `Str`/`f64` slots).

---

### 2026-09-19: deterministic variant-owner resolution reaches HIR lowering and LLVM codegen

- **`ModuleEnv::resolve_variant_owner_by_hint`** (`module_env.rs`): the
  verify-side deterministic resolution rule is now available without a Z3
  context — declared-type hint wins, sole/agreeing owners resolve, truly
  ambiguous names error listing the owners.
- **HIR lowering** (`hir.rs`): a bare `Name(args)` call converts to
  `VariantInit` only when the owner is unambiguous (sole owner or agreeing
  signatures) — a colliding bare name stays a `Call` and fails closed
  downstream instead of silently binding to a per-process HashMap pick.
- **LLVM codegen** (`mumei-emit-llvm`): `compile_pattern_test`,
  `bind_pattern_variables`, and the payload extractors resolve the owning
  enum through the match target's declared type — a bare parameter via
  `var_types` (which now records enum-typed parameters, not just structs),
  a `th.t` field access via the struct field's declared type, and other
  scrutinees via `infer_expr_type_name`. Nested variant field patterns
  propagate the field's declared type (`Self` resolves to the owner).
  Before this, `mumei build` could emit tag comparisons for a different
  enum than the one the verifier used when variant names collided
  (e.g. a user `enum Mine { Cons(i64), Nil }` vs the prelude `List`),
  silently diverging compiled code from verified semantics.
- **Tests**: `tests/test_codegen_enum_resolution.rs` pins the emitted tag
  constants for a prelude-colliding `match` on a declared parameter and on
  a struct field; 5 `ModuleEnv` unit tests cover hint wins, hint-misses,
  conflicting-tag ambiguity, agreeing signatures, and sole/unknown owners.
- **Docs**: `SPEC_GUIDE.md` "Finite enums and tagged unions" notes that
  codegen applies the same declared-type rule when emitting tag checks.

---

### 2026-09-19: parser — unknown atom-clause keywords are a checked-parse error

- **`mumei-core/src/parser/item.rs`**: the atom clause loop's catch-all
  silently skipped unrecognized tokens, so a malformed atom written as
  `atom name\n  inputs: x: i64\n  requires: true; …` parsed with zero
  params and the `inputs:`/`x:` lines dropped — the atom then **verified
  vacuously** (even `ensures: result >= 999` passed). The loop now records
  a `syntax_failure` for any word-like token followed by `:` that isn't a
  known clause keyword, so `parse_module_from_source_checked`
  (`verify`/`check`/`build` via `pipeline.rs`) rejects the file with
  `unknown atom clause keyword 'inputs:'` while the tolerant `parse_module`
  path (LSP) still recovers.
- **`mumei-core/src/parser/mod.rs`**: `ParseContext::syntax_failure` —
  non-`expect` syntax errors join `expect_failures`; `Token::is_word_like`
  covers `Ident` and alphabetic keyword tokens.
- Tests: `unknown_atom_clause_keyword_is_a_checked_parse_error` +
  `known_atom_clauses_do_not_fail_checked_parse`.

---

### 2026-09-18: P10-C — finite enums verify on native Z3 datatype sorts

- **`mumei-core/src/verification/support/datatype.rs` (new)**: a finite,
  non-recursive, non-generic enum whose payload fields all resolve to
  `i64`/`f64`/`Str`/`bool` lowers to a real Z3 `DatatypeSort` — variants are
  constructors with `is-<Variant>` testers and per-field selectors of the
  payload's true sort. Sorts are cached per context (`VCtx::enum_sorts`) so
  every use shares one datatype declaration.
- **Param/result seeding** (`param_z3_value_for_vc`): enum-typed params,
  `result`, and callee return placeholders in spec_validation, vacuity,
  call_graph, executor, and call-result seeding become `Datatype` consts.
  Enums with recursive/generic/enum/struct/array payloads stay on the Int-tag
  path.
- **`match` on datatype targets** (`pattern.rs`, `expr.rs`): arm coverage is
  the tester disjunction — `¬∨is_<V>(target)` UNSAT by the datatype
  completeness axiom — and a missing arm reports the constructor by name
  (`value = Blue -- no matching arm`). Field patterns bind to real selector
  applications, so `s == Named("hi")` proves `n == "hi"` in the `Named(n)`
  arm. `==`/`!=` between same-sort datatype operands is supported; ctor
  expressions `E::V(args)`, `E::V`, and bare `V` construct datatype values.
- **Fragment boundary** (`fragment.rs`): finite-ADT enum params and `match`
  expressions on finite ADTs no longer tag `inductive_data_type` — they stay
  in the decidable fragment. Recursive enums, generic enums, non-scalar
  payloads, and scalar-only `match`es keep the tag (Lean 4 delegation).
- **Tests**: `tests/test_datatype_enum{,_negative}.mm` + CLI harness (7 atoms
  incl. Str/f64/bool payload selectors, requires-side ctor equality, and a
  recursive-enum Int-tag pin); 6 fragment-classification unit tests.
- **Review hardening (self-review rounds 1–6)**: empty enums keep the
  Int-tag path (`DatatypeBuilder` panics on zero variants);
  `variant_selector_apply` indexes accessors safely; ctor misuse (arity
  mismatch, non-finite ctor, unknown variant on a known enum) is a hard
  `spec_lowering_failed` instead of a silently-skipped clause; non-finite
  `E::V` resolves to the tag index; comparing two different enum sorts is a
  spec error. **Variant-owner resolution is now deterministic** —
  `module_env.enums` is a `HashMap`, so a bare `Cons` pattern previously
  picked its tag index from whichever of `IntList`/prelude `List` came first
  in the hash order (same file verifying or failing per process).
  `resolve_variant_owner` resolves via the match target's declared type,
  accepts the unambiguous case, and fails closed when owners disagree;
  `detect_enum_from_arms`, counterexample decoding, bare-variant ctor
  errors, and fragment classification follow the same rule. Declared-type
  hints reach `match result` (the executor rebinds `result` to the body
  value, so the const name is lost — the hint comes from the scrutinee
  expression), bound recursive tails (`__proj_{Enum}_{Variant}_{i}`
  projector names carry the enum), and struct-field targets (`match th.t`
  resolves `th_t` against `th: Thermo`'s field type, including nested
  struct paths).
- **Tests**: `tests/test_datatype_enum{,_negative}.mm` + CLI harness (7 atoms
  incl. Str/f64/bool payload selectors, requires-side ctor equality, and a
  recursive-enum Int-tag pin); `test_enum_variant_resolution{,_negative}.mm`
  pins declared-type disambiguation and the fail-closed ambiguity error;
  6 fragment-classification unit tests.
- **Docs**: `SPEC_GUIDE.md` gains "Finite enums and tagged unions";
  `ROADMAP.md` marks P10-C complete.

---

### 2026-09-19: P10-B — regex contracts compile to Z3 RegLan (`str.in_re`)

- **`mumei-core/src/verification/support/reglan.rs` (new)**: a mini-compiler
  lowers the Rust `regex` syntax used by `matches` / `match_regex` /
  `re_match` literal patterns to native Z3 `Regexp` expressions. Substring
  (`is_match`) semantics are reconstructed as `Σ*·re·Σ*` with the `Σ*` elided
  on `^`-/`$`-anchored ends. Supported fragment: literals, concat, `|`, `()`,
  `*`/`+`/`?` (lazy included), `{n}`/`{n,}`/`{n,m}` (bounds ≤ 64), `[…]` /
  `[^…]` classes, `\d \w \s` (and `\D \W \S` complements), `\n \r \t`,
  `\xHH`, escaped metacharacters, `.` (any ASCII char except `\n`), and
  outermost anchors. `re.range` bounds above U+007F collapse in Z3 — range
  bounds past the ASCII plane are rejected as unsupported instead.
- **`effects.rs` where-clauses**: `matches(param, "pat")` now compiles to
  `param.in_re re` first; the previous prefix/suffix/contains approximations
  remain as a fallback for patterns outside the fragment.
- **`translator/expr.rs`**: `matches`/`match_regex`/`re_match` calls in
  atom contracts lower to `str.regex_matches(...)`. Non-literal patterns and
  uncompilable constructs are hard verification errors — a silently dropped
  `ensures` clause could otherwise report `verified` unchecked (偽陰性).
- **Fragment tagging** (`fragment.rs`, `proof_cert/validation.rs`):
  compilable `matches(`/`match_regex(`/`re_match(` calls no longer produce
  the `regex_semantics` tag or the `regex_semantics_require_manual_lemma`
  reason — both are kept only for patterns outside the decidable fragment
  (lookarounds, backreferences, inline flags, interior anchors, `\b`, …).
  `detects_regex_semantics` fixture coverage retained via `regex_match`.
- **Docs**: `docs/SPEC_GUIDE.md` gained a "String and regular-expression
  constraints" section describing the decidable fragment and the Lean
  delegation boundary; `docs/ROADMAP.md` marks P10-B complete.

---

### 2026-09-06: v0.6.19 release version bump

- **Workspace and member crate versions**: bumped versions from `0.6.18` to
  `0.6.19` so `mumei --version`, `mumei inspect`, and proof-certificate
  `mumei_version` match the release tag before tagging.
- **Install references**: synced the README and README_JA pinned install
  examples on `v0.6.19`, along with the Homebrew formula template and
  `install.sh` help examples.

---

### 2026-08-31: v0.6.18 release version bump

- **Workspace and member crate versions**: bumped versions from `0.6.17` to
  `0.6.18` so `mumei --version`, `mumei inspect`, and proof-certificate
  `mumei_version` match the release tag before tagging.
- **Install references**: synced the README and README_JA pinned install
  examples on `v0.6.18`, along with the Homebrew formula template and
  `install.sh` help examples.

---

### 2026-08-30: v0.6.17 release version bump

- **Workspace and member crate versions**: bumped versions from `0.6.16` to
  `0.6.17` so `mumei --version`, `mumei inspect`, and proof-certificate
  `mumei_version` match the release tag before tagging.
- **Install references**: synced the README and README_JA pinned install
  examples on `v0.6.17`, along with the Homebrew formula template and
  `install.sh` help examples.

---

### 2026-08-30: Z3 toolchain 4.13.4 → 5.1.0

- **Z3 5.1.0**: `mumei setup` and the Windows release job now install Z3 5.1.0,
  which brings the rewritten monadic regex solver plus soundness fixes in
  strings, quantified arrays, and bit-vectors — including cases that previously
  returned a spurious `unknown`.
- **Release-specific archive names**: upstream names each prebuilt archive after
  its build image and changes the suffix every release, so `src/setup.rs` no
  longer hard-codes `osx-13.7.1` / `glibc-2.35`; the OS, glibc suffixes, and
  minimum glibc are pinned per release in `Z3Build`. This also fixes the Linux
  aarch64 URL, which pointed at a `glibc-2.35` archive that upstream never
  published for 4.13.4 (`arm64` builds ship as `glibc-2.34`).
- **glibc fallback**: the 5.1.0 Linux archives import symbols up to
  `GLIBC_2.38`, so `mumei setup` detects the host glibc and installs Z3 4.14.1
  (`GLIBC_2.34`) on older distros instead of downloading an unusable binary.
  Hosts below even that floor, and hosts where the libc cannot be identified,
  no longer get a silently broken install.
- **`Z3_SYS_Z3_LIB_DIR` fix**: the generated `~/.mumei/env` pointed at
  `<toolchain>/lib`, which upstream archives do not contain — `libz3.so` and
  `libz3.a` sit in `bin/`. Linking against the `mumei setup` toolchain could
  not have worked. The env script now also exports a loader search path
  (`LD_LIBRARY_PATH` / `DYLD_FALLBACK_LIBRARY_PATH`), since `z3-sys` links
  libz3 dynamically, and omits every Z3 export when no bundled build is usable
  instead of shadowing the system install with dead paths.
- **Install verification**: `verify_installation` reported `✅` for any binary
  that merely spawned, so a prebuilt `z3`/`llc` that dies on a missing shared
  library printed a blank version as success; it now checks the exit status and
  surfaces the loader error. The "already installed" short-circuit checks for
  `bin/z3` rather than the directory, so a truncated toolchain re-installs.
- **Bindings unchanged**: `z3` 0.12 / `z3-sys` 0.8 build and pass the full
  `mumei-core` suite against libz3 5.1.0; the only C API removal in 5.x is
  unused by Mumei, so no crate upgrade (and no `Context` API migration) is
  needed.

---

### 2026-08-30: v0.6.16 release version bump

- **Workspace and member crate versions**: bumped versions from `0.6.12` to
  `0.6.16` so `mumei --version`, `mumei inspect`, and proof-certificate
  `mumei_version` match the release tag.
- **Release-tag consistency**: v0.6.13–v0.6.15 shipped while the manifests
  still said `0.6.12`, leaving `std-proof-bundle.json`'s tag-derived
  `mumei_version` inconsistent with the certificates inside it.
- **Install examples**: unified the README and README_JA pinned install
  examples on `v0.6.16`.
- **Release templates**: synced the Homebrew formula template and
  `install.sh` help examples.

---

### 2026-07-17: M2 trusted-atom reduction docs-sync + sorted-map regression certificates

- **Trusted-atom inventory sync**: `docs/TRUSTED_ATOMS.md` and
  `std/container/README.md` still described `std/container/sorted_map.mm::sorted_map_insert`
  as `trusted`, but its `trusted` body was already removed and the atom is now
  fully Z3-verified (append-at-end array-store obligation lowered to an explicit
  index range `0..map_len`, escalating to mumei-lean on `unknown`). Both docs now
  report **0 trusted atoms** in `std/` and mark Priority 2 complete.
- **Regression certificates**: added `tests/test_sorted_map_regression.mm`
  pinning the append (`keys[map_len] = key` preserves sortedness), remove-tail,
  and no-op removal proofs; wired into `build_and_run.sh` as a regression gate.
- **STDLIB_METRICS regeneration**: regenerated `docs/STDLIB_METRICS.md` with the
  mumei binary — 58 modules, 339 atoms (339 proven · 0 trusted), health 1.000.

---

### 2026-07-06: Trusted atom reduction, benchmark suite, and CROSS_PROJECT_ROADMAP sync

- **Trusted atom reduction**: removed `trusted` from `std/sorted_map.mm` atoms now that the Z3-decidable fragment covers their contracts; overall trusted-atom count reduced across the standard library.
- **Benchmark suite**: added `benchmarks/` directory with Dafny-style and SV-COMP-style verification benchmarks for comparing Z3 solver performance across contract styles; benchmark atom regex now supports `async` prefix atoms.
- **STDLIB_METRICS auto-update**: regenerated `docs/STDLIB_METRICS.md` with current atom/trusted counts using the mumei binary.
- **CROSS_PROJECT_ROADMAP sync**: updated `docs/CROSS_PROJECT_ROADMAP.md` to reflect translator v2 completion, `sorted_map` trusted atom removal, benchmark suite addition, and OTel Phase 1 status.

---

### 2026-07-05: P15 OpenTelemetry distributed tracing and OTel upgrade

- **P15 OpenTelemetry distributed tracing with TRACEPARENT propagation** (commit `1cd6117`): when built with `cargo build --features otel` and run with `OTEL_ENABLED=true`, `mumei verify` exports spans via OTLP. If `TRACEPARENT` is set in the environment (W3C Trace Context), the Rust spans become children of the caller's trace — enabling end-to-end distributed tracing from `mumei-agent` through the Z3 verification pipeline. Z3 span placement moved after early returns; `cli_timeout_ms` defaults to `-1` for unset.
- **OTel opentelemetry-rust 0.27→0.32 upgrade** (commit `2a4143a`): upgraded `opentelemetry-rust` from 0.27 to 0.32 so that W3C traceparent `flags=03` is accepted instead of silently rejected. This resolves W3C Trace Context interoperability when `mumei-agent` injects `TRACEPARENT` with both `sampled` and `random` flags.
- **CI pages deploy retry** (commit `0fc2097`): `deploy-pages` GitHub Actions step now retries on transient "Deployment failed" errors.
- **Roadmap docs sync**: updated `docs/ROADMAP.md` to note the opentelemetry 0.32 requirement and OTLP/HTTP endpoint for P15 trace penetration; added reference to mumei-agent's P15 OTLP stack and `OBSERVABILITY.md` for end-to-end trace verification.

---

### 2026-06-30: MCJIT→ORC docs-sync completion and sort ascending Lean escalation path

- **MCJIT→ORC docs-sync**: removed residual `(migrated from MCJIT)` parentheticals from `docs/ROADMAP.md` and `docs/CROSS_PROJECT_ROADMAP.md`; only the strikethrough+Resolved entry in Known Limitations remains as historical context.
- **Sort ascending Lean escalation fixture**: `tests/fixtures/sort_ascending.mm::verified_insertion_sort_ascending` provides an atom whose `forall(i, 0, n-1, arr[i] <= arr[i+1])` ensures triggers Z3 `unknown` (Array+forall quantifier timeout), making it a Lean escalation candidate. The mumei-lean bridge connects this to `MumeiLean.Sort.insertion_sort_ascending_bridge` backed by mathlib's `List.Sorted`.
- **Live generated theorem coverage**: updated from four to **five** paths across `README.md`, `docs/CROSS_PROJECT_ROADMAP.md`, and `docs/ROADMAP.md`.

---

### 2026-06-28: core-seeded deterministic forge and Lean bridge paths

- **vStd core predicates forge**: added `std/core_predicates.mm` from
  `forge_tasks/vstd_core_predicates.json` with explicit atom bodies and
  `deterministic_bodies: true`; `safe_index_or_zero`, `is_nonzero_flag`, and
  `preserve_safe_index` verify in the Z3-decidable fragment with no Lean
  escalation.
- **vStd core guards forge**: added `std/core_guards.mm` from
  `forge_tasks/vstd_core_guards.json` with explicit atom bodies and
  `deterministic_bodies: true`; `is_in_bounds`, `safe_abs_diff`,
  `clamp_to_positive`, and `both_positive` verify in the Z3-decidable
  fragment with no Lean escalation.
- **Crypto live Lean bridge fixture**: documented the third live generated
  theorem path, `std/crypto/primitives.mm::constant_time_eq_flag`, which lowers
  braced conditional body semantics to
  `Generated.Std.Crypto.Primitives.constant_time_eq_flag_correct` and exports
  `known_witness_used = false`.
- **Algebra finite-field live Lean bridge fixture**: documented the fourth live
  generated theorem path, `std/algebra/finite_field.mm::ff_zero_eq_zero`, which
  emits a mumei proof-certificate atom with `z3_result_class = "unknown"` for
  `ff_eq(result, 0, p)`, then lowers `ff_zero(p)` body semantics to
  `Generated.Std.Algebra.Finite_field.ff_zero_eq_zero_correct` and exports
  `known_witness_used = false`.

---

### 2026-06-28: vStd crypto primitives forge verification

- **vStd crypto primitives forge**: added `std/crypto/primitives.mm` from `forge_tasks/vstd_crypto_primitives.json` with `is_valid_key_len`, `is_valid_nonce_len`, `constant_time_eq_flag`, and `digest_len_ok`; `mumei verify --proof-cert std/crypto/primitives.mm` completes in the Z3-decidable fragment with no Lean escalation.

---

### 2026-06-28: Multi-language no-`.mm` audit contract sync

- **Multi-language no-`.mm` audit contract**: documented the canonical deterministic/no-LLM parser path for Python, Rust, TypeScript, and Go, including Rust `a + b` i64 overflow/bounds, TypeScript `name!.length` null/undefined, and Go `values[idx]` bounds findings normalized into `verification_violations` with Z3 counterexamples.
- **Contract vocabulary docs gate**: added the cross-project vocabulary check that keeps local docs aligned with `docs/CROSS_PROJECT_ROADMAP.md`, preserves the fixed no-`.mm` seven-key schema (`spec_health_issues`, `verification_violations`, `cross_validation_gaps`, `next_steps`, `migration_hints`, `healed_files`, `heal_errors`), and rejects legacy alias keys as public contract fields.
- **Lean bridge contract metadata**: kept the `translator_version` / `bridge_lemma_hash` synchronization requirement explicit so stale Lean translator output remains gated by `stale_translator` instead of being accepted as proof evidence.

---

### P9-D/E/F/G: NLAE integration completion

- **Loss Vector + structured feedback JSON**: `mumei verify --emit loss-vector <file.mm>` now emits P9-E structured feedback JSON, including reconstruction-loss details, violated property, counterexample values, source location, and an agent-facing repair instruction.
- **Self-Correction Protocol support**: verification failure reports can carry the Loss Vector into mumei-agent's P9-F self-correction loop, with `ENABLE_SELF_CORRECTION` enriching the feedback instruction for bounded retry workflows.
- **P9-G ecosystem fixture**: `examples/nlae_integration_demo.mm` provides the Module B (AR) side of the four-repository NLAE pipeline used by mumei-agent, mumei-lean, and mumei-demo.

---

### PR #221: Lean translator contract metadata

- **Typed translator metadata in proof certificates**: `AtomCertificate` now records `TranslatorIRMetadata` so Lean escalation bundles carry obligation sort, binders, theorem goal, provenance span, and lowering rules alongside the source contract.
- **Binder and bridge validation**: certificates include `binder_mapping` and `bridge_lemma_hash`, allowing mumei and mumei-lean to confirm that generated Lean binders and semantic bridge lemmas still match the compiler-side translator contract.
- **Manual lemma escalation reasons**: obligations that require hand-written Lean support carry `manual_lemma_reason`, keeping partial translations auditable instead of silently treating them as proof success.

---

### Plan 22: `mumei doc` enhancement + counter-example visualizer

#### `mumei doc` enhancement

- **Contract metadata in generated docs** (`src/main.rs`, `ItemDoc`): the
  doc generator now records `requires` / `ensures` / `body` / `effects`
  for every atom and renders them inline in the HTML and Markdown
  outputs. Trivial `requires: true` / `ensures: true` clauses are
  filtered out so the page stays focused on actual obligations.
- **Doc-comment extraction prefers `///`** (`extract_comment_before`):
  when a `///` doc-comment block immediately precedes an atom or type
  it is used verbatim; otherwise the existing `//` heuristic still
  applies, preserving backward compatibility with current `.mm`
  sources.
- **Syntax highlighting for contracts and bodies**
  (`highlight_mumei_code`): a small token-level highlighter wraps
  keywords, types, operators, numeric/string literals, and line
  comments in CSS classes (`kw`, `ty`, `op`, `num`, `str`, `cm`) so
  the generated HTML pages are readable without an external
  highlighter.
- **`--format json` mode** (`cmd_doc`): the doc subcommand now also
  accepts `json`, which streams the full structured documentation
  (atoms with all contract metadata, types, structs, enums, traits)
  to stdout and writes `<out>/docs.json` for static hosting and
  tooling integrations.
- **Client-side search** (`generate_html_docs`): the index page gains
  a search input that filters the module list incrementally.

#### Counter-example visualizer in editor

- **`MumeiError::VerificationError` carries a Z3 counter-example**
  (`mumei-core/src/verification.rs`): a new
  `counterexample: Option<serde_json::Value>` field is preserved
  through `with_source` / `with_help`, surfaced via `to_detail`,
  and populated by a `with_counterexample` builder. All five
  primary verification failure sites — postcondition violated,
  precondition violated at call site, trait law violated, match
  exhaustiveness, and division by zero — now thread their Z3 model
  values into the error.
- **LSP diagnostics expose the counter-example** (`src/lsp.rs`): the
  diagnostic message gains a `Counter-example: a = 1, b = 2` line
  and the structured value is also attached as `data.counterexample`
  on the LSP `Diagnostic`, so editors can render it without parsing
  the message string.
- **VS Code extension renders inline ghost-text decorations**
  (`editors/vscode/src/extension.ts`, `editors/vscode/CHANGELOG.md`,
  `editors/vscode/package.json` → `0.2.0`): a
  `TextEditorDecorationType` is registered for counter-example
  values and updated in response to `onDidChangeDiagnostics`,
  active-editor changes, and visible-editor changes, so the
  violating concrete inputs appear next to the failing line as
  italic ghost text.

---

### Plan 21: `trusted` reduction + real concurrency runtime

#### Verification

- **MIR move analysis** (`mumei-core/src/mir.rs`,
  `mumei-core/src/mir_analysis.rs`): nested `while` loops over `i64` (Copy)
  induction variables no longer trigger spurious `UseAfterMove` on
  `i = i + 1`. Numeric-literal type information is now propagated through
  HIR → MIR lowering so `lookup_movability` correctly classifies the local
  as `Copy` at the back-edge merge.
- **Z3 forall in `ensures`** (`mumei-core/src/verification.rs`): the
  `expr_to_z3` handler for `Expr::Call("forall", …)` now extracts
  `select(arr, idx)` patterns from the body and attaches them to the
  generated `forall_const`, mirroring the existing `requires`-side
  pattern extraction. Combined with `set_bool("mbqi", true)` on the
  solver `Params` for atoms that contain array-quantified constraints,
  this enables E-matching to fire on post-store array states so that
  `ensures: forall(i, 0, n, arr[i] >= 0)` is provable when the same
  forall is present in `requires` and the body's stores preserve it.
- `trusted` removed from `tests/test_array_store.mm::test_array_store_loop`,
  `tests/test_verified_sort.mm::verify_noop_sort` and
  `verify_insertion_sort_skeleton`, and from `std/list.mm::insertion_sort` /
  `verified_insertion_sort` / `verified_merge_sort` (the move-analysis
  reason; the full sortedness-preservation Z3 proof remains a separate
  task tracked in `docs/CROSS_PROJECT_ROADMAP.md`).
- New regression tests:
  - `tests/test_array_forall_ensures.mm` — covers pattern-based
    E-matching for `forall` in `ensures` over arrays modified by
    stores.
  - `tests/test_nested_while_no_trusted.mm` — covers the
    nested-while + Copy-typed induction-variable case.

#### LLVM IR codegen — concurrency runtime

- **`task` / `task_group:all`** (`mumei-emit-llvm/src/codegen.rs`):
  each `task { body }` now emits a separate
  `__mumei_task_<atom>_<N>(i8*) → i8*` wrapper function. The parent
  marshals the body's i64 free variables into a stack-allocated args
  struct, then emits `pthread_create(&thread, NULL, wrapper, &args)` and
  `pthread_join(thread, NULL)` calls. The wrapper loads captures from
  the args struct, runs the body, and stores the result back into the
  trailing slot of the struct, which the parent reads after join.
  `task_group:all` lowers as **spawn-all-then-join-all**:
  `emit_task_spawn_only` is called for every child first, then
  `emit_task_join_only` joins each `PendingTask` in declaration order.
  This is what makes the children actually concurrent — the
  alternative spawn-join-spawn-join layout would deadlock simple
  `recv` / `send` rendezvous patterns inside the group. A regression
  test `chan_rendezvous_in_group` exercises exactly this case.
  `task_group:any` now waits on the first completed child via a runtime
  completion flag, cancels the remaining children, and joins them for cleanup.
- **Diagnostic for dropped task captures**: when a `task` body
  references a parent-scope free variable that the IR closure
  conversion can't yet marshal (anything that isn't `i64` — `f64`,
  `Str`, struct, pointer, array fat-pointer), the codegen emits an
  `eprintln!` warning naming the dropped variable and the enclosing
  atom so users notice immediately rather than silently observing a
  zero in the wrapper. Threading this through `MumeiError` for a
  proper user-facing diagnostic is a follow-up.
- **`send` / `recv`**: lowered to direct calls to the runtime helpers
  `__mumei_chan_send(i64, i64)` and `__mumei_chan_recv(i64) → i64`.
- **Runtime library** (`runtime/mumei_runtime.c`): provides
  `__mumei_chan_send` / `__mumei_chan_recv` / `__mumei_chan_init`,
  backed by a fixed table of `pthread_mutex_t` + `pthread_cond_t`
  channel slots. The send/recv pair implements single-slot rendezvous
  semantics — `send` waits if a value is already pending; `recv` waits
  until one arrives.
- New integration test `tests/test_concurrency_runtime.mm` exercises
  `task`, `task_group:all`, and `send` / `recv` codegen end-to-end.
  `examples/concurrent_http.mm` is annotated to clarify that the
  concurrency structure now compiles to real `pthread_*` calls.

#### Documentation

- `docs/ARCHITECTURE.md` — LLVM IR Codegen table updated with `task`,
  `task_group:all`, `send`, `recv` rows.
- `docs/CONCURRENCY.md` — Implementation Status table updated:
  Runtime scheduler and Channel types are now implemented; LLVM
  codegen rows split into `task` / `chan`; `task_group:any` flagged
  as not yet implemented.

---

### `Stmt::ArrayStore`: first-class `arr[i] = v` across parser → verifier → codegen

#### Language / AST
- **`Stmt::ArrayStore { array, index, value, span }`**: new statement variant
  for array-element assignment. `parser::expr::parse_statement` detects the
  `<expr>[idx] = <expr>` pattern at statement level and promotes it from a
  value expression to `Stmt::ArrayStore`.
- `Stmt::span()` extended to cover the new variant.

#### HIR / MIR / codegen
- `HirStmt::ArrayStore` mirrors the AST variant; `lower_stmt_with_env` lowers
  `Stmt::ArrayStore` into it.
- `mumei-core/src/mir.rs` lowers `HirStmt::ArrayStore` into a fresh
  `Place::Index(Place::Local(arr), idx_temp)` assignment, materializing the
  index in a temporary local.
- `mumei-emit-llvm/src/codegen.rs` emits a GEP + `store i64` guarded by a
  signed-less-than / non-negative bounds check. OOB writes are skipped
  (branch-to-merge) rather than aborting the process, matching the
  `ArrayAccess` read-side fallback.
- `mumei-emit-llvm/src/binary.rs::rename_calls_in_hir_stmt` learned the new
  arm and recurses into `index` / `value`.

#### Verification (Z3)
- `stmt_to_z3` handles `Stmt::ArrayStore` by:
  1. Lowering `index` / `value` to `z3::ast::Int`.
  2. Running the same OOB check used by `ArrayAccess` against `len_<name>` —
     any satisfiable `!(0 <= idx < len_<name>)` aborts with a targeted
     "Potential Out-of-Bounds store" error.
  3. Updating the environment-stored Z3 array under the **per-array**
     `__z3_arr_<name>` key using `Array::store(current, idx, val)`.
     `expr_to_z3::ArrayAccess` reads the matching `__z3_arr_<name>` key
     (falling back to `vc.arr`) so subsequent `arr[j]` reads observe the
     store — `select(store(a, i, v), i) = v`. Keying by name is required for
     soundness when multiple arrays (e.g. `arr` and `aux`) appear in the same
     body: a single shared key would let a store to `brr` pollute reads from
     `arr`. Regression: `tests/test_array_store_multi.mm`.
- All AST-`match` traversals in `verification.rs`
  (`collect_acquire_resources_stmt`, `collect_callees_with_args_stmt`,
  `collect_array_accesses_in_stmt`, `collect_callees_stmt`,
  `collect_divisors_stmt`, `body_has_symbolic_perform_args`, async-depth
  self-call counter) grew `ArrayStore` arms that recurse into `index` and
  `value`.
- `ast::Monomorphizer::collect_from_stmt` and
  `hir::collect_free_variables_stmt` gained matching arms so generics and
  free-variable analysis still cover array-store bodies.

#### Standard library
- `std/list.mm`: `verified_insertion_sort` replaced with a real nested-while
  insertion-sort body using `arr[i] = val` (key variable + inner-loop shift
  + outer-loop increment). Marked `trusted` — the `arr[i] = val` store
  tracking itself is checked, but full `forall(i, 0, n-1, arr[i] <= arr[i+1])`
  functional correctness remains future work (Z3 Array + forall quantifiers
  still timeout on this pattern). `verified_merge_sort` updated to a
  divide-and-conquer skeleton with the same `trusted` boundary.

#### Tests
- `tests/test_array_store.mm`: 4 positive atoms (constant-index store,
  ループ内 ゼロ埋め[trusted], swap, variable-index store) — all verified.
- `tests/negative/array_store_oob.mm`: `arr[n] = 42` correctly rejected with
  the new OOB-store error.
- `tests/test_verified_sort.mm`: noop-sort and insertion-sort skeleton
  (both `trusted`) — regression for parser / HIR / MIR / LLVM pipelines on
  real nested-loop store bodies.
- `build_and_run.sh`: three new example-test entries for the above files.

---

### Plan 7 follow-up: Windows / musl release build stabilization

#### Windows (`x86_64-pc-windows-msvc`)
- `release.yml`: LLVM 17 source tarball download now retries up to 5 times
  with exponential backoff and a 10 MB minimum-size sanity check, addressing
  intermittent 504 Gateway Timeout responses from the GitHub release CDN.
- Z3 4.13.4 prebuilt download gained equivalent retry + size guard.

#### musl (`x86_64-unknown-linux-musl`)
- `release.yml`: Alpine Docker build now installs both stable and nightly
  Rust toolchains and invokes `cargo +nightly build -Zhost-config
  -Ztarget-applies-to-host` with a `.cargo/config.toml` that applies
  `target-feature=-crt-static` to `[host]` (build scripts) and
  `target-feature=+crt-static` to `[target.x86_64-unknown-linux-musl]`
  (final binary).
- This fixes the bindgen panic "Unable to find libclang: ... Dynamic loading
  not supported" that previously occurred when the build script was linked
  fully static and could not `dlopen` libclang inside Alpine musl.
- `z3/static-link-z3` keeps libz3 statically linked into the final binary.

#### CI
- `allow-failure: true` removed from the Windows matrix entry; the
  `continue-on-error: ${{ matrix.allow-failure || false }}` expression on
  the job is also removed.  Both targets must now pass for the release job
  to succeed.

---

### Emitter Artifact Abstraction + CHeaderEmitter Doxygen + Contextual Suggestions

#### Task A: Emitter trait Artifact abstraction (Roadmap #5 Phase 1)
- Added `Artifact` struct (`name: PathBuf`, `data: Vec<u8>`, `kind: ArtifactKind`) and `ArtifactKind` enum (`Binary`, `Source`, `Header`) to `mumei-core/src/emitter.rs`
- Changed `Emitter` trait return type from `MumeiResult<()>` to `MumeiResult<Vec<Artifact>>`
- `LlvmEmitter::emit()`: reads generated `.ll` file back as `Artifact` (Phase 1 — `codegen::compile()` internals unchanged)
- `CHeaderEmitter::emit()`: removed `std::fs::write()`, returns content as `Artifact`
- `cmd_build` in `src/main.rs`: receives `Vec<Artifact>` and writes each artifact to disk

#### Task B: CHeaderEmitter Doxygen format enhancement
- Changed comment format: `/* requires: ... */` → `/** @pre ... */`, `/* ensures: ... */` → `/** @post ... */`
- Added `@brief` auto-generated from atom name
- Expanded `mumei_type_to_c()`: added `i32` → `int32_t`, `u32` → `uint32_t`, `f32` → `float`

#### Task C: Dynamic suggestion generation (report.json accuracy)
- Added `build_contextual_suggestion()` in `mumei-core/src/verification.rs`: generates context-aware fix suggestions from `failure_type`, `counterexample`, and `structured_unsat_core`
- Integrated into `save_visualizer_report()` and `build_semantic_feedback()` with `suggestion_for_failure_type()` fallback

#### Tests
- `mumei-core/src/emitter.rs`: 8 unit tests (type mappings, Doxygen format, artifact kind/path, header guard, full output format)
- `mumei-core/src/verification.rs`: 7 unit tests for `build_contextual_suggestion()` (precondition/postcondition/division-by-zero/invariant with counterexamples, fallback behavior)

#### Documentation
- Updated `docs/CROSS_PROJECT_ROADMAP.md`: Phase 1 marked as ✅ Implemented with Artifact abstraction and Doxygen details
- Updated `docs/REPORT_SCHEMA.md`: `suggestion` field description updated to note dynamic suggestions
- Updated `docs/CHANGELOG.md`

---

### P2-A: Cross-atom Contract Composition (enhanced) — Chained Calls & E2E Tests

#### New unit tests
- `test_cross_atom_composition_chained_abc`: verifies chained A→B→C call propagation (open_file → read_file → write_and_close)
- `test_cross_atom_composition_effect_post_available_to_caller`: verifies callee's `effect_post` is available to caller's subsequent `perform` operations

#### New E2E test
- `tests/test_cross_atom_chain.mm`: chained cross-atom composition with 3-atom pipeline (open_file → read_file → write_and_close)

#### Documentation
- Updated `docs/CROSS_PROJECT_ROADMAP.md`: P2-A marked as ✅ Implemented with full feature list
- Updated `docs/ROADMAP.md` Plan 24: "not yet implemented" → "now implemented via `analyze_temporal_effects_with_contracts()`"

---

### P2-A: Cross-atom Contract Composition + P2-B: Trait Method Constraints Z3 Injection

#### Cross-atom Contract Composition (P2-A)
- Extended `analyze_temporal_effects()` in `mumei-core/src/mir_analysis.rs` to handle `Rvalue::Call` statements
- Added `AtomEffectContract` struct mapping effect names to (pre_state, post_state) pairs
- Added `TemporalOp` enum to distinguish between `Perform` and `Call` operations
- New `analyze_temporal_effects_with_contracts()` function: forward dataflow analysis now verifies callee `effect_pre` against caller's current temporal state and applies `effect_post` as state transition
- Updated `src/verification.rs` to build `callee_contracts` map from `ModuleEnv` and pass to the new analysis function
- Added 3 unit tests: `test_cross_atom_composition_valid`, `test_cross_atom_composition_invalid_order`, `test_cross_atom_composition_no_contracts`
- Updated `tests/test_modular_verification.mm`: updated comments for `full_pipeline`
- Added `tests/test_modular_verification_error.mm`: `bad_pipeline` atom (invalid order test case) in separate file

#### Trait Method Constraints Z3 Injection (P2-B)
- Added `div` method to `Numeric` trait with `param_constraints: vec![None, Some("v != 0")]`
- Added `get_trait_for_method()` helper to `ModuleEnv` for looking up trait method constraints by method name
- Implemented param_constraints injection in `expr_to_z3()` for inter-atom calls: detects trait impl methods, substitutes constraint variables, and verifies with Z3 solver (push/assert(not)/check/pop pattern)
- Implemented param_constraints injection in `verify_impl()`: asserts method parameter constraints as solver preconditions during law verification
- Added `tests/test_trait_constraints.mm` with `SafeDiv` trait, `safe_divide` (should pass), and `unsafe_divide` (should fail)

#### Review Fixes
- **Law verification soundness**: Moved `param_constraints` injection inside `solver.push()`/`solver.pop()` scope per law, and added `law_expr.contains(&method.name)` filter to prevent unrelated constraints (e.g., `div`'s `b != 0`) from weakening verification of laws like `commutative_add`
- **Trait method name collision guard**: Added `find_impl(trait_name, callee_type)` check at call sites to prevent user-defined atoms named `div`, `add`, etc. from having builtin trait constraints spuriously applied
- **E2E test separation**: Moved `bad_pipeline` (intentionally failing atom) to `tests/test_modular_verification_error.mm` to prevent `mumei check` from failing on the main test file
- **Naive string replace TODO**: Documented fragility of `constraint.replace("v", param_name)` at both injection sites with TODO for future word-boundary-aware replacement

#### Documentation
- Updated `docs/ARCHITECTURE.md`: Modular Verification section now documents cross-atom composition and trait method constraints
- Updated `docs/CHANGELOG.md`: this entry (with review fixes)

---

### Proposal A: `--report-dir` option for `mumei verify`

- Added `--report-dir <dir>` CLI option to `mumei verify` to specify report.json output directory
- Eliminates race condition when multiple concurrent verify calls write to the same cwd
- Creates the target directory automatically when `--report-dir` is specified
- Updated `mcp_server.py` to use `--report-dir` in all verify call sites (`validate_logic`, `execute_mm`)
- Backward compatible: defaults to current directory when `--report-dir` is omitted

### Proposal B: `--json` option for `mumei verify`

- Added `--json` flag to `mumei verify` for stdout JSON output
- When `--json` is active, all human-readable output (emoji, miette diagnostics) is suppressed
- Informational messages in `load_and_prepare` (monomorphization, FFI Bridge) use `eprintln!` to avoid stdout corruption
- Outputs report.json content to stdout, or minimal JSON status if no report file is produced
- Enables pipeline integration: `mumei verify --json file.mm | jq '.semantic_feedback'`
- Follows same pattern as existing `cmd_inspect_file --json` implementation

### Plan 22: PII Data Pipeline Example

- Added `examples/pii_pipeline.mm`: Valid PII anonymization pipeline demonstrating compile-time enforcement
- Added `examples/pii_pipeline_error.mm`: Intentionally invalid pipeline showing `InvalidPreState` detection
- Added `tests/test_pii_pipeline.mm`: E2E integration test for PII pipeline
- Added 3 unit tests in `src/mir_analysis.rs` for DataPipeline state machine verification

### Plan 23: Regex Path Policies + URL Validation

- Added `RegexSafeFileRead(path: Str) where matches(path, "^/tmp/[a-z]+/.*")` to `std/effects.mm`
- Added `SecureHttpGet`/`SecureHttpPost` with `starts_with(url, "https://")` constraint to `std/http.mm`
- Added `examples/regex_path_policy.mm`: Regex-based path constraint demo
- Added `examples/secure_http.mm`: HTTPS enforcement demo
- Added `tests/test_regex_policy.mm`: E2E test for regex path validation
- Added `tests/test_url_validation.mm`: E2E test for URL validation
- Improved Z3 regex approximation: exact match (`^literal$`) and prefix+suffix (`^prefix.*suffix$`) patterns

### Plan 24: Modular Verification (effect_pre / effect_post)

- Added `effect_pre`/`effect_post` fields to `Atom` struct in `src/parser/ast.rs`
- Added parser support for `effect_pre: { Key: Value };` / `effect_post: { Key: Value };` syntax
- Updated all Atom construction sites across codebase (main.rs, resolver.rs, ast.rs, mir.rs, mir_analysis.rs, verification.rs)
- `effect_pre` overrides initial state of state machines during temporal verification
- `effect_post` checked against exit states; mismatch emits `UnexpectedFinalState` error
- Invalid state names in `effect_pre`/`effect_post` now produce hard errors (not silently ignored)
- Missing state machines emit warnings; missing exit states (no perform ops) emit warnings
- Monomorphizer substitutes effect type variables in `effect_pre`/`effect_post` keys (e.g., `{ E: Closed }` → `{ FileWrite: Closed }`)
- Added 3 unit tests for modular verification in `src/mir_analysis.rs`
- Added 3 parser tests for effect_pre/effect_post in `src/parser/mod.rs`
- Added `tests/test_modular_verification.mm`: E2E test with File effect contracts (includes NOTE about cross-atom composition limitation)
- Updated `docs/ARCHITECTURE.md`: "Modular Verification (Future)" → "Modular Verification (Implemented)"

---

## PR #83: Plans 15–20 — Examples, FFI Memory, Str Migration, Codegen Types, MIR Migration, Z3 Integration

### Summary

Implements Plans 15–20 of the Mumei compiler roadmap: example files, FFI memory management, Str type migration, LLVM codegen return type improvements, MIR Phase 4c completion, and Z3 temporal effect integration.

### Plan 15 — Examples + E2E Tests

- 5 example files: `http_demo.mm`, `json_demo.mm`, `str_demo.mm`, `enum_payload.mm`, `concurrent_http.mm`
- 3 test files: `test_json_operations.mm`, `test_str_type.mm`, `test_enum_payload.mm`

### Plan 16 — FFI Memory Management

- `json_free()`, `string_free()`, `http_free()` FFI functions added to release handles from global stores
- `mumei_str_alloc()` / `mumei_str_free()` / `mumei_str_get()` for managed string lifetime
- Exposed as `free()` / `str_free()` atoms in `std/json.mm` and `std/http.mm`
- HTTP `alloc_string_result` deduplicated — delegates to `json.rs` implementation

### Plan 17 — Str Type Migration

- Examples updated to use `Str`-typed parameters for string arguments (URLs, keys, etc.)

### Plan 18 — LLVM Codegen Return Type Improvements

- `Atom.return_type: Option<String>` field added (parser + monomorphizer)
- `-> Type` syntax parsed after atom parameter list (e.g., `atom greet(name: Str) -> Str`)
- `resolve_return_type()` replaces hardcoded i64 with annotation-driven type resolution
- Callee call-site return type resolved from callee's `return_type` annotation
- Match phi nodes infer type from first arm's body value (not hardcoded i64)
- Unreachable block value matches inferred phi type (float/pointer/struct/int)

### Plan 19 — MIR Phase 4c Completion (Documentation)

- MIR `MoveAnalysis` is now the primary ownership/move engine
- `LinearityCtx` retained only for Z3-level borrow tracking
- Comment/documentation updates across `mir.rs`, `mir_analysis.rs`, `verification.rs`

### Plan 20 — Temporal Effect Z3 Integration

- `encode_effect_state()` maps state names to integers for Z3 Int Sort
- `ConflictingState` at merge points now uses scoped Z3 solver probe:
  - UNSAT → hard error (irreconcilable conflict)
  - SAT → info diagnostic (compatible states)
  - Unknown → warning (solver timeout)
- Constraint budget check before Z3 probe creation
- 3 unit tests for state encoding and Z3 satisfiability

### CI Fixes

- musl and Windows release builds marked `allow-failure` with `continue-on-error`
- Windows Z3 installation changed from slow vcpkg source build to pre-built release binary
- musl build sets `CC=musl-gcc` for correct C compiler

### Files Changed

| File | Summary |
|---|---|
| `src/parser/ast.rs` | `Atom.return_type: Option<String>` field |
| `src/parser/item.rs` | `-> Type` return type parsing |
| `src/codegen.rs` | `resolve_return_type()`, callee return type resolution, match phi type inference, unreachable block type fix |
| `src/ast.rs` | Monomorphizer propagates `return_type` |
| `src/main.rs` | ExternFn→Atom `return_type` propagation (3 sites) |
| `src/resolver.rs` | ExternFn→Atom `return_type` propagation |
| `src/mir.rs` | `return_type: None` in test helpers, Phase 4c documentation |
| `src/mir_analysis.rs` | `return_type: None` in test helpers, Phase 4c documentation |
| `src/verification.rs` | `encode_effect_state()`, Z3 ConflictingState probe, Phase 4c documentation, 3 tests |
| `src/ffi/json.rs` | `json_free`, `string_free`, `mumei_str_alloc/free/get`, handle counter fix |
| `src/ffi/http.rs` | `http_free`, deduplicated `alloc_string_result` |
| `std/json.mm` | `free()`, `str_free()` atoms + extern declarations |
| `std/http.mm` | `free()` atom + extern declaration |
| `examples/*.mm` | 5 new example files |
| `tests/*.mm` | 3 new test files |
| `.github/workflows/release.yml` | musl/Windows `allow-failure`, pre-built Z3 for Windows |

### Test Results

- All 201+ tests passing

---

## Task 3: Temporal Effect Verification (Stateful Effects)

### Summary

Implements compile-time verification of effect state transitions (temporal ordering).
Effects can now define states (e.g., Closed, Open) and transitions (e.g., open: Closed → Open),
and the compiler verifies that operations occur in valid states using forward dataflow analysis
on the MIR CFG.

### Stateful Effect Syntax (Parser Extensions)

- **`EffectDef`** gains `states: Vec<String>`, `transitions: Vec<EffectTransition>`, `initial_state: Option<String>`
- **`EffectTransition`** struct: `operation`, `from_state`, `to_state`
- Parser recognizes `states: [...]`, `initial: ...`, `transition op: From -> To;` inside effect definitions
- Backward compatible: empty states = stateless effect (existing behavior unchanged)

### EffectStateMachine (mir_analysis.rs)

- `EffectStateMachine`: Constructed from `EffectDef`, holds states, transition map, initial state
- `can_transition(operation, current_state)` / `next_state(operation, current_state)` methods
- `MAX_EFFECT_STATES = 8`: Effects with more states are skipped with a warning

### Forward Dataflow Analysis (mir_analysis.rs)

- `analyze_temporal_effects()`: Worklist algorithm tracking effect state through MIR CFG
- `EffectStateMap`: `HashMap<effect_name, current_state>` per basic block
- Violation types: `InvalidPreState`, `ConflictingState`, `UnexpectedFinalState`
- Iteration limit: `block_count * max(state_machines_count, 10)`

### Verification Pipeline (Phase 1i)

- Phase 1i added to `verify_inner()`: builds state machines from `effect_defs`, runs `analyze_temporal_effects()`
- `InvalidPreState` / `UnexpectedFinalState` → hard verification errors
- `ConflictingState` → warnings (Z3 delegation marked as TODO for future)
- Metrics recorded for Phase 1i timing

### Modular Verification Stubs

- TODO comments added to `Atom` struct for future `effect_pre` / `effect_post` fields
- Documents the modular verification approach for cross-atom state tracking

### Files Changed

| File | Summary |
|---|---|
| `src/parser/ast.rs` | `EffectDef` extended with states/transitions/initial_state, `EffectTransition` struct |
| `src/parser/item.rs` | Stateful effect syntax parsing (states, initial, transition keywords) |
| `src/parser/mod.rs` | 3 parser tests for stateful effect syntax |
| `src/mir_analysis.rs` | `EffectStateMachine`, `analyze_temporal_effects()`, 9 unit tests |
| `src/verification.rs` | Phase 1i temporal effect verification, `register_builtin_effects` defaults |
| `tests/test_temporal_effects.mm` | Integration test with stateful File effect |
| `std/effects.mm` | Stateful effect example (commented) |
| `docs/ROADMAP.md` | Phase 7 (Temporal Effect Verification) added |
| `docs/ARCHITECTURE.md` | Phase 1i + Stateful Effects section added |
| `docs/CHANGELOG.md` | This entry |

### Test Results

- All tests passing (existing + 12 new: 9 unit tests + 3 parser tests)

---

## PR #77: Task 4 — Effect Parameter Z3 String Sort Integration

### Summary

Integrates Z3's native String Sort (`z3::ast::String`) for symbolic verification of effect parameter constraints. Previously, only constant (literal) effect arguments were verified; variable/symbolic arguments were silently skipped. This PR adds a hybrid verification strategy and includes cumulative fixes from Tasks 0 (explosion prevention), 1 (move analysis), and 2 (liveness + drop).

### Z3 String Sort for Effect Parameters

- **`parse_constraint_to_z3_string()`**: Maps constraint strings to Z3 String operations:
  - `starts_with(path, "/tmp/")` → `Z3String::prefix_of`
  - `ends_with(path, ".txt")` → `Z3String::suffix_of`
  - `contains(path, "data")` → `Z3String::contains`
  - `not_contains(path, "..")` → `NOT Z3String::contains`
- **Perform handler extended**: Symbolic (variable) args create Z3 String variables with unique IDs per call site (`EFFECT_STR_COUNTER`) and assert effect constraints
- **Sort-aware timeout**: Two-pass pre-scan (`body_has_symbolic_perform_args`) doubles Z3 timeout when String constraints are detected
- **Constraint budget**: String constraint creation tracked against per-atom budget (default: 1000)
- **`not_contains` support**: Added to `evaluate_string_constraint` and `check_constant_constraint` for parity

### MIR Infrastructure (Tasks 0, 1, 2)

- **`src/mir_analysis.rs`** (new): Liveness analysis (backward dataflow), drop insertion, and forward dataflow move analysis
  - `compute_gen_kill()` / `compute_liveness()` / `insert_drops()`: Backward dataflow for automatic resource cleanup
  - `analyze_moves()`: Forward dataflow detecting UseAfterMove, DoubleMove, ConflictingMerge
  - `MirLinearityState`: Per-local alive/consumed tracking with merge conflict detection
- **While-loop MIR off-by-one fix**: `header_id = ctx.next_block + 1` (was self-loop)
- **Iteration bounds**: Liveness and move analysis use `block_count * max(local_count, 10)` for correct convergence
- **MIR analysis budget**: `MIR_ANALYSIS_COMPLEXITY_LIMIT = 10,000` prevents explosion on pathological inputs
- **ConflictingMerge**: Reported as warnings (not hard errors) pending Copy vs Move type distinction (Phase 4c)

### Verification Pipeline Improvements

- **Constraint budget exceeded**: Correctly classified as `"constraint_budget_exceeded"` failure type (was misclassified as precondition violation)
- **Metrics**: `VerificationMetrics` tracks per-phase timing and constraint counts
- **`evaluate_string_constraint`**: Now handles `not_contains` (was conservatively allowing unknown constraints)

### Files Changed

| File | Summary |
|---|---|
| `src/verification.rs` | Z3 String Sort integration, `parse_constraint_to_z3_string()`, sort-aware timeout pre-scan, constraint budget fix, ConflictingMerge warnings, `not_contains` support |
| `src/mir.rs` | While-loop off-by-one fix, TODO comments for nested control flow fragility |
| `src/mir_analysis.rs` | **New** — Liveness analysis, drop insertion, move analysis with correct iteration bounds |
| `docs/ARCHITECTURE.md` | Z3 String Sort section, verification steps updated |
| `docs/ROADMAP.md` | Z3 String Sort integration status updated |
| `docs/CHANGELOG.md` | This entry |

### Test Results

- 181 tests passing (175 existing + 6 new Z3 String Sort tests)
- New tests: `test_constant_path_ok`, `test_constant_path_ng`, `test_z3_string_parse_constraint_starts_with`, `test_z3_string_constraint_satisfiability`, `test_contains_constraint`, `test_z3_string_performance`

---

## PR #69: Phase 4a Wiring + HIR Effect Types (Task 5) + Capability Security (Task 6)

### Summary

Completes Phase 4a LinearityCtx wiring, adds HIR effect type information, and evaluates capability security for the parameterized effect system.

### Part A — LinearityCtx Wiring (Phase 4a completion)

- `VCtx` gains `linearity_ctx` and `effect_ctx` fields (wrapped in `RefCell` for interior mutability)
- `check_alive()` wired into `expr_to_z3` Variable branch for use-after-consume detection
- `borrow()`/`release_borrow()` wired into call-site ref/ref-mut argument handling
- Removed `#[allow(dead_code)]` from `borrow`, `release_borrow`, `check_alive`

### Part B — HIR Effect Type Information (Task 5)

- New types: `HirEffectSet` (`BTreeSet<String>` for deterministic iteration), `HirEffectUsage`
- Added `effect_set` to `HirAtom`, `callee_effects` to `HirExpr::Call`, `effect_usage` to `HirExpr::Perform`
- New `lower_atom_to_hir_with_env()` populates effect info from `ModuleEnv`
- Codegen + all 3 transpilers read from `hir_atom.effect_set.effects` with `atom.effects` fallback for parameterized detail

### Part C — Capability Security (Task 6)

- `verify_effect_params()` and `verify_effect_consistency()` wired into `verify_inner()`
- `EffectCtx` wired into `VCtx` and `Perform` handling in `expr_to_z3`
- `SecurityPolicy` field added to `ModuleEnv`; `is_effect_allowed` check in Perform handler
- `build_effect_feedback()` wired into `verify_effect_containment()` error path (human-readable explanation)
- `docs/CAPABILITY_SECURITY.md`: evaluation document recommending Option A (parameterized effects + Z3)

### Test Results

- 140 existing Rust unit tests pass
- New test `.mm` files: `test_borrow_tracking.mm`, `test_use_after_consume.mm`, `test_capability_evaluation.mm`

---

## Four-Task Implementation: Parser Migration + Extern Codegen + Span Fix + MIR Foundation

### Summary

Implements four interconnected roadmap tasks as a single cohesive change: item parser regex→recursive descent migration, LLVM codegen for extern functions, import span mismatch fix, and MIR foundation with LinearityCtx wiring.

### Task 1: Item Parser regex → Recursive Descent

- **`src/parser/item.rs`**: Rewrote `parse_module_from_source()` and `parse_atom_from_source()` to use token-based parsing via `Lexer` + `ParseContext` instead of ~20 `Regex::new()` calls
- Smart token-to-text reconstruction with `append_token()` helper for context-aware spacing
- All 134 existing tests pass unchanged — backward compatible

### Task 2: LLVM Codegen for Extern Functions (P1-A Completion)

- **`src/codegen.rs`**: Added `declare_extern_functions()` that emits LLVM IR `declare` statements for each `ExternFn`
- Maps Mumei types → LLVM types via `resolve_param_type()`
- Sets calling convention based on `extern_block.language` ("C" or "Rust")
- **`src/main.rs`**: Added `collect_extern_blocks()` helper to gather `ExternBlock` items for codegen
- **`docs/ROADMAP.md`**: Marked P1-A LLVM codegen as ✅

### Task 3: Import Span Mismatch Fix

- **`src/main.rs`**: Created `resolve_source_for_span()` helper that checks `Span.file` and reads the imported file when needed
- Fixed all 5+ locations where `e.with_source(&source, &atom.span)` used the wrong source for imported atoms/impls
- Removed all TODO comments related to the span mismatch bug

### Task 4a: LinearityCtx Wiring into Verification Pipeline

- **`src/verification.rs`**: Added `linearity_ctx: Option<&'a RefCell<LinearityCtx>>` field to `VCtx` struct
- Wrapped `LinearityCtx` in `RefCell` for interior mutability (avoids refactoring 60+ call sites)
- Wired `check_alive()` into `expr_to_z3` Variable branch (use-after-consume detection)
- Wired `borrow()` into call-site `ref`/`ref mut` argument handling
- Wired `consume()` into call-site `consumed_params` argument handling
- `LinearityCtx.borrow()`, `check_alive()`, `consume()` are no longer dead code

### Task 4b: MIR Data Structures + HIR → MIR Lowering

- **`src/mir.rs`** (new): MIR data structures (`Local`, `Place`, `Rvalue`, `Operand`, `MirConstant`, `MirStatement`, `Terminator`, `BasicBlock`, `MirBody`, `LocalDecl`)
- `lower_hir_to_mir()`: Flattens nested HIR expressions into three-address code across BasicBlocks
  - `HirStmt::Let` → `MirStatement::Assign` + `StorageLive`
  - `HirExpr::BinaryOp` → temp + `Rvalue::BinaryOp`
  - `HirExpr::IfThenElse` → 3+ BasicBlocks with `Terminator::SwitchInt`
  - `HirStmt::While` → loop header / body / after blocks with back-edge
  - `HirExpr::Call` → `Rvalue::Call`
- 6 unit tests covering addition, if/else, let binding, function call, while loop, constants
- **`src/hir.rs`**: Updated TODO comment to reference `src/mir.rs`
- **`src/main.rs`**: Added `mod mir;`

### Files Changed

| File | Summary |
|---|---|
| `src/parser/item.rs` | Regex → recursive descent migration with smart token reconstruction |
| `src/codegen.rs` | `declare_extern_functions()` for LLVM IR extern declarations |
| `src/main.rs` | `resolve_source_for_span()`, `collect_extern_blocks()`, `mod mir;` |
| `src/verification.rs` | LinearityCtx wired into VCtx via RefCell, borrow/consume/check_alive at call sites |
| `src/mir.rs` | **New** — MIR data structures + HIR → MIR lowering + 6 unit tests |
| `src/hir.rs` | Updated MIR TODO comment |
| `docs/ROADMAP.md` | P1-A LLVM codegen ✅, Phase 4 status updated |
| `docs/ARCHITECTURE.md` | Pipeline diagram + source file table updated |
| `docs/CHANGELOG.md` | This entry |

### Test Results

- 140 tests passing (134 original + 6 new MIR tests)

---

## PR #62: Parser Migration — Recursive Descent with Proper Lexer

### Summary

Migrates the parser from regex-based approach to a full recursive descent parser with proper lexer. Replaces monolithic `src/parser.rs` (3,052 lines) with 7 focused modules under `src/parser/`. Also incorporates PR #61's `contract()` clause parsing for higher-order function parameters.

### Parser Module Structure

| Module | Role |
|---|---|
| `src/parser/mod.rs` | Public API, `ParseContext` struct, 84+ tests |
| `src/parser/token.rs` | `Token` enum (60+ variants), `SpannedToken` with line/col/len |
| `src/parser/lexer.rs` | `Lexer` — source string → `Vec<SpannedToken>` with span tracking |
| `src/parser/ast.rs` | All AST types (`Expr`, `Stmt`, `Item`, `Atom`, etc.) |
| `src/parser/expr.rs` | Pratt parser for expressions (operator precedence via binding power) |
| `src/parser/item.rs` | Recursive descent for top-level items (replaces ~15 regex patterns) |
| `src/parser/pattern.rs` | Match arm pattern parsing |

### Key Changes

- **Lexer**: Proper tokenization with span tracking (line/col/len per token), handles comments, string literals, multi-character operators (`==`, `!=`, `>=`, `<=`, `=>`, `&&`, `||`, `|>`, `->`)
- **Pratt Parser**: Extensible operator precedence via binding power table — trivial to add `|>`, `@`, future operators
- **Item Parsing**: All top-level items (import, type, struct, enum, trait, impl, resource, effect, extern, atom) parsed via recursive descent instead of regex
- **contract() Clause**: `Param.fn_contract_requires` and `Param.fn_contract_ensures` fields for higher-order function parameter contracts (from PR #61)
- **Keyword Field Access**: Keywords like `mode`, `priority` correctly handled as field names after `.` and as function names in expression contexts
- **Backward Compatible**: All public APIs preserved (`parse_module`, `parse_expression`, `parse_body_expr`, `parse_atom`, `tokenize`) — zero caller changes needed

### Unblocks

- Phase 2: Basic Effect System (`<E: Effect>` generic syntax)
- Phase C: Lambda syntax (`fn(x) => x + 1`)
- MIR introduction (CFG with accurate source spans)

---

## PR #61: call_with_contract — Z3 Verification of Higher-Order Functions

### Summary

Implements `call_with_contract` in the verification engine so that higher-order functions (`map`, `fold_left`, `result_map`, etc.) can be formally verified by Z3 without `trusted` markers.

### Key Changes

- **`contract(f)` clause syntax**: Declare requires/ensures constraints for function parameters
- **Phase B verification**: `CallRef` dynamic case expands contracts via Z3 (requires validation + ensures assertion)
- **Removed `trusted`** from `map`, `fold_left`, `list_map`, `result_map`, `apply`, `apply_twice`, `fold_two`
- **Documentation**: `instruction.md` §3.5 — contract syntax reference

### Test Results

- `tests/test_call_with_contract.mm`: 10/10 atoms verified
- `std/option.mm`: 8/8 verified (including `map` without `trusted`)
- `std/result.mm`: 12/12 verified (including `result_map` without `trusted`)
- `std/list.mm`: `fold_left` and `list_map` verified without `trusted`

---

## Effect System: Inference, Refinement Types × Effects, Hierarchy

### Summary

Implements comprehensive Effect Inference and Refinement Types × Effects integration with a hybrid verification approach (Constant Folding + Symbolic String ID).

### Effect System Core

- **Effect/EffectDef structs**: `Effect` with params, `EffectDef` with constraint and `parent:` for hierarchy
- **Parser**: `effects: [...]` clause in atoms, `effect` declarations with `parent:` and `where` constraints
- **Effect Hierarchy (Subtyping)**: `parent:` field enables Network → HttpRead/TcpConnect relationships
- **`get_effect_ancestors()` / `is_subeffect()`**: Traverse hierarchy chain for subtype checking

### Effect Inference

- **`infer_effects()`**: Call graph traversal infers required effects from callee atoms
- **`infer_effects_json()`**: JSON serialization for CLI/MCP integration
- **`verify_effect_consistency()`**: Checks declared vs inferred effects with subtyping support

### Hybrid Path Verification

- **Constant Folding**: Rust-side compile-time check for literal path constraints (e.g., `starts_with`)
- **Symbolic String ID**: Path strings mapped to Z3 Int sort for variable path verification
- **`verify_effect_params()`**: Effect parameter constraint verification

### CLI & MCP

- **`mumei infer-effects <file>`**: New CLI subcommand for JSON effect inference output
- **`get_inferred_effects` MCP tool**: Pre-check tool for AI to verify required permissions before writing code

### File Consistency

- All transpilers (Rust/Go/TypeScript) output effect annotations as doc comments
- LLVM IR codegen includes effect metadata
- LSP hover displays effect information
- Resolver handles `Item::EffectDef` registration with FQN support
- `compute_atom_hash()` includes effect fields for cache invalidation

### Documentation

- **ROADMAP.md**: Z3 String Sort migration plan + Effect hierarchy extensions
- **ARCHITECTURE.md**: Updated verification steps (1f, 1g) and pipeline diagram
- **instruction.md**: Updated coding conventions for `Item::EffectDef` and `Atom.effects`

---

## PR #35: miette Rich Diagnostics (Phase 1) + Higher-Order Functions `atom_ref` (Phase 2)

### Summary

Replaces plain-text error output with [miette](https://crates.io/crates/miette)-powered rich diagnostics (colored output, source code context, underline highlighting, actionable suggestions). Introduces first-class function references via `atom_ref(name)` and indirect invocation via `call(f, args...)` with automatic contract propagation through Z3.

### Phase 1: miette Integration

- `MumeiError` now derives `thiserror::Error` + `miette::Diagnostic` with `NamedSource`, `SourceSpan`, and `#[help]`
- `span_to_source_span()` converts line/col/len `Span` → byte-offset `SourceSpan` (handles `\n` and `\r\n`)
- Builder methods `.with_source()` and `.with_help()` for post-hoc error enrichment
- `original_span: Span` field preserved for LSP backward compatibility
- Help suggestions for: postcondition violations, precondition violations, division by zero, out-of-bounds, refinement type predicates

### Phase 2: Higher-Order Functions (Phase A)

- **AST**: `Expr::AtomRef { name }` and `Expr::CallRef { callee, args }` variants
- **Parser**: `atom_ref(name)` and `call(expr, args...)` in `parse_primary()`; depth-tracking parenthesis parser for nested `atom_ref(i64, i64) -> i64` in parameter lists
- **Verification**: `CallRef` resolves `atom_ref(concrete_name)` contracts; parametric function-type parameters return unconstrained symbolic values (atoms must be `trusted`)
- **Codegen**: `AtomRef` → function pointer via `ptr_to_int` with lazy forward declaration; `CallRef` → direct call optimization or `build_indirect_call`
- **Transpilers**: Function type mapping for Rust (`fn(T) -> R`), Go (`func(T) R`), TypeScript (`(arg: number) => number`)
- **Std library**: `map` (option.mm), `fold_left`/`list_map` (list.mm), `result_map` (result.mm) — all `trusted` for Phase A
- **Example**: `examples/higher_order_demo.mm`

### Bug Fixes

- Fixed `to_detail()` LSP regression — `original_span` field restores `Span` propagation
- Fixed `parse_atom` regex `[^)]*` — replaced with depth-tracking parenthesis parser
- Fixed `collect_callees` missing `AtomRef`/`CallRef` match arms (cycle detection)
- Fixed `count_self_calls` missing `AtomRef`/`CallRef` match arms (async recursion depth)
- Fixed `collect_acquire_resources` missing `AtomRef`/`CallRef` match arms (BMC resource safety)
- Fixed `body_contains_float` missing recursion into `CallRef` args (Rust transpiler)
- Fixed codegen forward reference — `AtomRef` lazily declares functions not yet in LLVM module
- Fixed indirect call f64 type — inspects actual argument types via `is_float_value()`

### Files Changed

| File | Summary |
|---|---|
| `src/verification.rs` | `MumeiError` restructured with miette derives, `span_to_source_span`, `original_span`, `AtomRef`/`CallRef` in `expr_to_z3`/`collect_callees`/`count_self_calls` |
| `src/parser.rs` | `AtomRef`/`CallRef` expr variants, depth-tracking param parser, `split_params`, fn type parsing |
| `src/codegen.rs` | `AtomRef` → `ptr_to_int` with lazy declare, `CallRef` → direct/indirect call |
| `src/main.rs` | miette handler init, `load_and_prepare` returns source, `with_source` at all error sites |
| `src/ast.rs` | `TypeRef::fn_type()`, `is_fn_type()`, monomorphizer `AtomRef`/`CallRef` traversal |
| `src/transpiler/*.rs` | Function type mapping, `AtomRef`/`CallRef` formatting |
| `src/lsp.rs` | `to_detail()` now uses `original_span` for LSP positioning |
| `Cargo.toml` | Added `miette` (v7, fancy) and `thiserror` (v2) |
| `std/option.mm` | `trusted atom map` with `atom_ref` parameter |
| `std/result.mm` | `trusted atom result_map` with `atom_ref` parameter |
| `std/list.mm` | `trusted atom fold_left`/`list_map` with `atom_ref` parameters |
| `examples/higher_order_demo.mm` | **New** — `atom_ref` + `call` demonstration |
| `docs/DIAGNOSTICS.md` | Rewritten for miette integration (English) |
| `README.md` | Higher-order functions section, rich diagnostics showcase, roadmap updates |

---

## PR #32: Strategic Roadmap v0.3.0+ — Full Implementation (P1 + P2 + P3)

### Summary

Implements all 3 priorities from the strategic roadmap defined in PR #31.
Network-first standard library, runtime portability, and CLI tools.

### Implementation Highlights

| Priority | Phase | Implementation |
|---|---|---|
| P1-A | FFI Bridge | `src/main.rs` + `src/resolver.rs`: extern → trusted atom auto-registration |
| P1-B | std.json | `std/json.mm`: 19 atoms (parse, stringify, get, array, object) |
| P1-C | std.http | `std/http.mm`: 11 atoms (get, post, put, delete, status, body) + reqwest dependency |
| P1-D | Integration Demo | `examples/http_json_demo.mm`: task_group + HTTP + JSON parallel processing |
| P2-A | CI Portability | `release.yml`: LLVM 17 apt setup + dependency libraries (aarch64-linux planned) |
| P2-B | Homebrew | `scripts/homebrew/mumei.rb`: Formula template |
| P2-C | WebInstall | `scripts/install.sh`: curl \| sh installer |
| P3-A | REPL | `src/main.rs`: `mumei repl` command (interactive execution) |
| P3-B | Doc Gen | `src/main.rs`: `mumei doc` command (HTML/Markdown auto-generation) |
| P3-C | Integration | `:load std/http.mm` in REPL → HTTP atoms available |

### Files Changed

| File | Summary |
|---|---|
| `src/main.rs` | FFI Bridge, `mumei repl`, `mumei doc` commands |
| `src/resolver.rs` | ExternBlock → trusted atom registration (via import) |
| `Cargo.toml` | inkwell fix (0.5.0), reqwest added |
| `std/json.mm` | **New** — JSON operations standard library (19 atoms) |
| `std/http.mm` | **New** — HTTP client standard library (11 atoms) |
| `examples/http_json_demo.mm` | **New** — task_group + HTTP + JSON integration demo |
| `scripts/install.sh` | **New** — curl \| sh installer |
| `scripts/homebrew/mumei.rb` | **New** — Homebrew Formula template |
| `.github/workflows/release.yml` | LLVM 17 apt setup + dependency libraries |
| `docs/STDLIB.md` | std.json, std.http reference updated |
| `docs/ROADMAP.md` | Status updated to Implemented |
| `docs/CHANGELOG.md` | This changelog entry |

---

## PR #31: Strategic Roadmap v0.3.0+ (docs update)

### Summary

Defines 3 strategic roadmap priorities to evolve Mumei from an experimental language to a practical tool.
All related documentation updated with priorities, dependencies, and timelines.

### 3 Strategic Priorities

| Priority | Theme | Key Deliverable |
|---|---|---|
| 🥇 P1 | Network-First Standard Library | FFI Bridge + std.json + std.http |
| 🥈 P2 | Runtime Portability | Static linking + Homebrew + WebInstall |
| 🥉 P3 | CLI Developer Experience | mumei repl + mumei doc |

### Files Changed

| File | Summary |
|---|---|
| `docs/ROADMAP.md` | **New** — Detailed strategic roadmap (Phase A–D, dependencies, success metrics, timeline) |
| `README.md` | Added std.json, Runtime Portability, REPL, doc gen to Roadmap section |
| `instruction.md` | §11 rewritten as Strategic Roadmap v0.3.0+ (3 priorities) |
| `docs/TOOLCHAIN.md` | Future Roadmap updated to 3-priority table format |
| `docs/FFI.md` | Added FFI Bridge Completion implementation plan to future extensions |
| `docs/CONCURRENCY.md` | Added std.http integration demo + Task refinement items |
| `docs/STDLIB.md` | Added Planned: std/json.mm + std/http.mm sections |
| `docs/CHANGELOG.md` | This changelog entry |

---

## PR #16 (feature/alloc → develop)

### Summary

This PR implements dynamic memory management, ownership system, borrowing, and completes the remaining roadmap items (except LSP) for the Mumei language.

---

## Phase 1–3: Standard Prelude Foundation

- **`std/prelude.mm`**: `Eq`/`Ord`/`Numeric` traits with Z3 laws, `Option<T>`/`Result<T,E>`/`List<T>`/`Pair<T,U>` ADTs, `Sequential`/`Hashable` abstract interfaces
- **`src/resolver.rs`**: `resolve_prelude()` for auto-import
- **`src/main.rs`**: Prelude auto-loading in `load_and_prepare()`

## Phase 4: Trait Method Refinement Constraints

- `TraitMethod.param_constraints` field in `src/parser.rs`
- Syntax: `fn div(a: Self, b: Self where v != 0) -> Self;`
- `Numeric` trait gains `div` with zero-division prevention

## Phase 5: Law Body Expansion

- `substitute_method_calls()` in `src/verification.rs`
- Word-boundary-aware `replace_word()` substitution
- `split_args()` for nested parenthesis handling
- Error messages now show expanded law expressions

## Phase 6: Dynamic Memory (alloc)

- **`std/alloc.mm`**: `RawPtr`, `NullablePtr`, `Owned` trait, `Vector<T>`, `HashMap<K,V>`
- **`src/verification.rs`**: `LinearityCtx` — ownership + borrowing tracking
- **`src/codegen.rs`**: `alloc_raw` → `malloc`, `dealloc_raw` → `free` (LLVM IR)

## Ownership & Borrowing

- **`consume` modifier**: `Atom.consumed_params` parsed from `consume x;` syntax
- **`ref` keyword**: `Param.is_ref` parsed from `ref v: T` syntax
- **Z3 integration**: `__alive_` / `__borrowed_` symbolic Bools
- **LinearityCtx**: `register()`, `consume()`, `borrow()`, `release_borrow()`, `check_alive()`
- **Transpiler**: Rust `ref` → `&T`, TypeScript `ref` → `/* readonly */`

## HashMap\<K, V\>

- `struct HashMap<K, V> { buckets, size, capacity }` with field constraints
- 11 verified atoms: `map_new`, `map_insert`, `map_get`, `map_contains_key`, `map_remove`, `map_size`, `map_is_empty`, `map_rehash`, `map_drop`, `map_insert_safe`, `map_should_rehash`

## Equality Ensures Propagation

- `ensures: result == n + 1` now propagates through chained calls
- `propagate_equality_from_ensures()` recursively extracts `result == expr` from `&&`-joined ensures

## FQN Dot-Notation

- `math.add(x, y)` resolved as `math::add` in both verification and codegen
- Automatic `.` → `::` conversion

## Incremental Build

- `.mumei/cache/verification_cache.json` with enhanced per-atom verification cache
- `compute_proof_hash()`: hashes `name | requires | ensures | body_expr | consume | ref | effects | trust | callee signatures | type predicates`
- Transitive dependency tracking: callee contract changes automatically invalidate callers
- `VerificationCacheEntry`: stores `proof_hash`, `result`, `dependencies`, `type_deps`, `timestamp`
- Old `.mumei_build_cache` automatically migrated via `migrate_old_cache()`
- Unchanged atoms skip Z3 verification
- Cache invalidation on verification failure

## Nested Struct Support

- `v.point.x` resolved via recursive `build_field_path()`
- Path flattening: `["v", "point", "x"]` → `v_point_x` / `__struct_v_point_x`
- LLVM codegen: recursive `extract_value` chains

## Struct Method Definitions

- `StructDef.method_names` field for FQN registration as `Stack::push`

## Negative Test Suite

8 test files in `tests/negative/`:

| File | Tests |
|---|---|
| `postcondition_fail.mm` | ensures violation |
| `division_by_zero.mm` | zero-division detection |
| `array_oob.mm` | out-of-bounds access |
| `match_non_exhaustive.mm` | non-exhaustive match |
| `consume_ref_conflict.mm` | ref + consume conflict |
| `invariant_fail.mm` | loop invariant initial failure |
| `requires_not_met.mm` | inter-atom precondition violation |
| `termination_fail.mm` | non-decreasing ranking function |

---

## Files Changed

| File | Summary |
|---|---|
| `std/prelude.mm` | Traits, ADTs, interfaces, alloc reference |
| `std/alloc.mm` | **New** — Vector, HashMap, ownership primitives |
| `src/parser.rs` | `param_constraints`, `consumed_params`, `is_ref`, `method_names` |
| `src/verification.rs` | LinearityCtx, law expansion, equality propagation, nested struct, FQN |
| `src/codegen.rs` | malloc/free, FQN dot-notation, nested extract_value |
| `src/resolver.rs` | Prelude auto-load, incremental build cache |
| `src/main.rs` | Prelude integration, incremental build in verify/build |
| `src/transpiler/rust.rs` | `ref` → `&T` |
| `src/transpiler/typescript.ts` | `ref` → `/* readonly */` |
| `tests/negative/*.mm` | 8 negative test files |
| `README.md` | Full documentation update |
| `docs/STDLIB.md` | **New** — Standard library reference |
| `docs/CHANGELOG.md` | **New** — This file |

---

## Remaining Roadmap (pipeline integration pending)

The following data structures and logic are implemented but not yet wired into the compiler pipeline:

| Item | Data Structure | Status |
|---|---|---|
| Struct method parsing | `StructDef.method_names` | ⏳ Parser for `impl Stack { atom push(...) }` syntax |
| Trait method constraints | `TraitMethod.param_constraints` | ⏳ Z3 injection in `verify_impl` and inter-atom calls |
| Automatic borrow tracking | `LinearityCtx.borrow()` / `release_borrow()` | ✅ Integrated (PR #69) |
| Use-after-consume detection | `LinearityCtx.check_alive()` | ✅ Integrated (PR #69) |
| Effect tracking context | `EffectCtx` | ✅ Integrated (PR #69) |
| Security policy enforcement | `SecurityPolicy` | ✅ Integrated (PR #69) |
| Effect consistency check | `verify_effect_consistency()` | ✅ Integrated (PR #69, warning level) |
| Effect parameter constraints | `verify_effect_params()` | ✅ Integrated (PR #69) |
| Effect feedback | `build_effect_feedback()` | ✅ Integrated (PR #69) |
