# 🏗️ Mumei Compiler Architecture

## Pipeline

```
source.mm → parse → resolve → monomorphize → lower_to_hir → verify (Z3) → codegen (LLVM IR) → transpile (Rust/Go/TS)
                                                    ↑
                                      HIR: Expr/Stmt separation, type annotation
                                      Resource Hierarchy Check (deadlock-free proof)
                                      Effect Containment Check (side-effect safety proof)
                                      Async Safety Verification (ownership across await)
                                      // TODO: → lower_to_mir → borrow check → verify
```

## Source Files

| File | Role |
|---|---|
| `src/parser/` | Modular recursive descent parser with proper lexer |
| `src/parser/mod.rs` | Public API (`parse_module`, `parse_expression`, `parse_body_expr`, `parse_atom`, `tokenize`), `ParseContext` struct |
| `src/parser/token.rs` | `Token` enum (60+ variants), `SpannedToken` with line/col/len tracking |
| `src/parser/lexer.rs` | `Lexer` struct — converts source string to `Vec<SpannedToken>` with span info |
| `src/parser/ast.rs` | All AST type definitions (`Expr`, `Stmt`, `Item`, `Atom`, `StructDef`, `EnumDef`, etc.) |
| `src/parser/expr.rs` | Expression/statement parsing with Pratt parser (operator precedence via binding power table) |
| `src/parser/item.rs` | Top-level item parsing — recursive descent replacements for all regex patterns, `contract()` clause parsing for higher-order function parameters |
| `src/parser/pattern.rs` | Pattern parsing for match arms |
| `src/ast.rs` | `TypeRef`, `Monomorphizer` — generic type expansion engine |
| `src/resolver.rs` | Import resolution, circular detection, prelude auto-load, incremental build cache |
| `src/verification.rs` | Z3 verification, `ModuleEnv`, `LinearityCtx`, law expansion, equality propagation, resource hierarchy, BMC, async recursion depth, inductive invariant, trust boundary, `call_with_contract` (Phase B higher-order function verification) |
| `src/codegen.rs` | LLVM IR generation — Pattern Matrix, StructType, malloc/free, nested extract_value |
| `src/hir.rs` | HIR (High-level IR) definitions, AST → HIR lowering |
| `src/transpiler/` | Multi-target: Rust (`&T`), Go (interface), TypeScript (`/* readonly */`) |
| `src/main.rs` | CLI orchestrator — `build`/`verify`/`check`/`init` with incremental cache |

---

## ModuleEnv

Zero global state. All definitions in one struct:

```rust
pub struct ModuleEnv {
    pub types: HashMap<String, RefinedType>,
    pub structs: HashMap<String, StructDef>,
    pub atoms: HashMap<String, Atom>,
    pub enums: HashMap<String, EnumDef>,
    pub traits: HashMap<String, TraitDef>,
    pub impls: Vec<ImplDef>,
    pub verified_cache: HashSet<String>,
    pub resources: HashMap<String, ResourceDef>,
    pub effects: HashMap<String, EffectDef>,
    pub effect_defs: HashMap<String, EffectDef>,       // Full registry with hierarchy
    pub path_id_map: HashMap<String, i64>,              // Symbolic String ID (hybrid approach)
    pub next_path_id: i64,
    pub prefix_ranges: HashMap<String, (i64, i64)>,     // Path prefix → ID range
    pub dependency_graph: HashMap<String, HashSet<String>>, // atom → callees
    pub reverse_deps: HashMap<String, HashSet<String>>,     // atom → callers
}
```

---

## LinearityCtx (Ownership + Borrowing)

```rust
pub struct LinearityCtx {
    alive: HashMap<String, bool>,           // true=alive, false=consumed
    borrow_count: HashMap<String, usize>,   // 0=free, 1+=borrowed
    borrowers: HashMap<String, Vec<String>>,
    violations: Vec<String>,
}
```

**Errors detected:**
- `Double-free detected: 'x' has already been consumed`
- `Use-after-free detected: 'x' has been consumed`
- `Cannot consume 'x': currently borrowed by [y]`
- `Cannot consume ref parameter 'x'`
- `Cannot borrow 'x': it has already been consumed`

---

## Verification Steps (per atom)

1. Quantifier constraints (`forall`/`exists`)
2. Refinement type injection (params → Z3 symbolic variables)
2b. Struct field constraints (recursive for nested structs)
2c. Array length symbols (`len_<name> >= 0`)
2d. Linearity setup (`__alive_`/`__borrowed_` Z3 Bools)
2e. Effect allowed set injection (`__effect_allowed_*` Z3 Bools)
3. `requires` assertion
4. Body evaluation (`stmt_to_z3` / `expr_to_z3` — Expr/Stmt separated)
5. `ensures` verification (negate + check Sat)
6. Equality ensures propagation (`result == expr` → Z3 equality)
7. Linearity finalization (consume marking + violation check)
8. Contradiction check

---

## Law Verification (verify_impl)

1. Build method body map from `impl`
2. Build parameter name map from `trait` methods
3. For each law: `substitute_method_calls()` expands `add(a,b)` → `(a + b)`
4. Parse expanded expression and verify with Z3
5. If Sat (law violated): show counter-example with expanded form

---

## Incremental Build (Enhanced Verification Cache)

- **Cache file**: `.mumei/cache/verification_cache.json`
- **Entry format**: `VerificationCacheEntry { proof_hash, result, dependencies, type_deps, timestamp }`
- **Proof hash**: `SHA256(name | requires | ensures | body_expr | consume | ref | effects | trust | callee signatures | type predicates)`
  - Includes transitive callee signatures (requires/ensures) — if a callee's contract changes, all callers are automatically re-verified
  - Includes type predicate content for refined type parameters
- **Cache hit** → skip Z3 verification, mark as verified
- **Cache miss** → re-verify, update cache on success
- **Failure** → remove from cache (force re-verify next time)
- **Migration**: Old `.mumei_build_cache` files are automatically migrated to the new format

---

## FQN Resolution

- `math.add(x, y)` → `math::add` (automatic `.` → `::` conversion)
- Applied in both `expr_to_z3`/`stmt_to_z3` (verification) and `compile_hir_expr`/`compile_hir_stmt` (codegen)
- Resolver registers both `add` and `math::add` in ModuleEnv

---

## Transpiler Mapping

| Mumei | Rust | Go | TypeScript |
|---|---|---|---|
| `atom f(x: T)` | `pub fn f(x: T)` | `func f(x T)` | `function f(x: number)` |
| `ref x: T` | `x: &T` | `x T // ref` | `/* readonly */ x: number` |
| `ref v: T` | `v: &T` | `v T // ref` | `/* readonly */ v: number` |
| `ref mut v: T` | `v: &mut T` | `v *T` | `/* &mut */ v: number` |
| `consume x` | move semantics | comment | comment |
| `enum E { A, B }` | `enum E { A, B }` | `const + type` | `const enum + union` |
| `struct S { f: T }` | `struct S { f: T }` | `type S struct` | `interface S` |
| `trait T { fn m(); }` | `trait T { fn m(); }` | `type T interface` | `interface T` |

---

## Nested Struct Resolution

`v.point.x` is resolved by:

1. `build_field_path()` → `["v", "point", "x"]`
2. Try env lookup: `__struct_v_point_x`, `v_point_x`
3. If not found: recursively evaluate inner expression
4. LLVM codegen: chain `extract_value` calls

---

## Ownership & Borrowing (Aliasing Prevention)

### Borrow Modes

| Modifier | Z3 Tracking | Semantics |
|---|---|---|
| (none) | `__alive_` Bool | Owned value. Can be consumed via `consume x;` |
| `ref` | `__borrowed_` Bool | Shared read-only reference. Multiple `ref` allowed simultaneously |
| `ref mut` | `__exclusive_` Bool | Exclusive mutable reference. No other `ref` or `ref mut` to same data |
| `consume` | `__alive_` → false | Ownership transfer. Use-after-free detected by LinearityCtx |

### Aliasing Prevention (Z3)

When `ref mut` exists, the verifier checks all other `ref`/`ref mut` params of the same type:

```
∀ p1, p2 ∈ params:
  p1.is_ref_mut ∧ p1.type == p2.type ∧ p1 ≠ p2
  → Z3.assert(p1 ≠ p2)  // if SAT (may be equal), report aliasing error
```

Example:
```mumei
// ✅ OK: different types
atom safe(ref mut x: i64, ref y: f64) ...

// ✅ OK: requires proves they are distinct
atom safe2(ref mut x: i64, ref y: i64)
requires: x != y;
...

// ❌ ERROR: ref mut x and ref y may alias (same type, no distinctness proof)
atom unsafe(ref mut x: i64, ref y: i64)
requires: true;
...
```

---

## Async/Await + Resource Hierarchy (Concurrency Safety)

### Design

Mumei treats concurrency safety as a **compile-time verification problem**.
Instead of relying on runtime deadlock detection, the compiler uses Z3 to
mathematically prove that resource acquisition order is safe.

### Resource Definition

```mumei
resource db_conn priority: 1 mode: exclusive;
resource cache   priority: 2 mode: shared;
```

Each resource has:
- **Priority**: Defines the acquisition order. Higher priority = acquired later.
- **Mode**: `exclusive` (write, no concurrent access) or `shared` (read-only, concurrent OK).

### Deadlock Prevention (Resource Hierarchy)

**Invariant**: If thread T holds resource L₁ and requests L₂, then `Priority(L₂) > Priority(L₁)`.

The verifier (`verify_resource_hierarchy`) encodes this as Z3 constraints:
1. For each pair of resources (rᵢ, rⱼ) where i < j in the declaration order
2. Assert `Priority(rⱼ) > Priority(rᵢ)`
3. If Z3 finds a counterexample (SAT), report a deadlock risk

### Data Race Prevention (Ownership Model)

Resources in `exclusive` mode enforce single-writer semantics:
- `HasAccess(Thread, Resource, Write)` → no other thread may access
- `HasAccess(Thread, Resource, Read)` → other threads may also read (shared mode)

### Syntax

```mumei
// Atom with resource declaration
atom transfer(amount: i64)
resources: [db_conn, cache];
requires: amount >= 0;
ensures: result >= 0;
body: {
    acquire db_conn {
        acquire cache {
            amount + 1
        }
    }
};

// Async atom
async atom fetch_data(id: i64)
requires: id >= 0;
ensures: result >= 0;
body: {
    let result = await get_remote(id);
    result
};
```

### Await Safety Verification

The `Expr::Await` handler performs two critical checks at each suspension point:

**1. Resource Held Across Await (Deadlock Prevention)**

If `await` is called inside an `acquire` block, the resource lock is held during
suspension — a classic deadlock pattern. The verifier scans `env` for any
`__resource_held_*` keys that are `true` and reports an error:

```
❌ Unsafe await: resource 'db_conn' is held across an await point.
   Hint: acquire db_conn { ... }; let val = await expr; // OK
   Bad:  acquire db_conn { let val = await expr; ... }  // deadlock risk
```

**2. Ownership Consistency Across Await**

Variables consumed (`__alive_` = false) before an `await` point are marked with
`__await_consumed_*` flags. This enables detection of use-after-free patterns
where a consumed variable is accessed after the coroutine resumes.

### Bounded Model Checking (BMC)

For loops containing `acquire` expressions, BMC unrolls the loop up to
`BMC_UNROLL_DEPTH` (default: 3) iterations and verifies resource ordering
at each step. This catches bugs like:

```mumei
// BMC detects: acquire order reversal across iterations
while cond invariant: true {
    acquire cache { acquire db { ... } }  // iteration N: cache → db
    // iteration N+1: cache → db (OK, same order each time)
}
```

BMC is a **complement** to loop invariants, not a replacement:
- Loop invariants provide **complete** proofs (∀ iterations)
- BMC provides **bounded** proofs (first N iterations only)
- If no invariant is provided, BMC acts as a safety net

### Trust Boundary (trusted / unverified)

External code that hasn't been verified by Mumei can be explicitly marked:

```mumei
// FFI wrapper: contract is trusted, body is not verified
trusted atom ffi_read(fd: i64)
requires: fd >= 0;
ensures: result >= 0;
body: fd;

// Legacy code: warning emitted, partial verification attempted
unverified atom legacy_process(x: i64)
requires: x >= 0;
ensures: result >= 0;
body: x + 1;

// Combination: async + trusted
async trusted atom fetch_external(url: i64)
requires: url >= 0;
ensures: result >= 0;
body: url;
```

Trust levels:
- **Verified** (default): Full Z3 verification of body, requires, ensures
- **Trusted**: Body verification skipped; contract (requires/ensures) assumed correct
- **Unverified**: Warning emitted; verification attempted only if contract is non-trivial

### Inductive Invariant Verification

For recursive async atoms, `invariant:` provides **complete** proofs (vs BMC's bounded proofs):

```mumei
async atom process(state: i64)
invariant: state >= 0;
requires: state >= 0;
ensures: result >= 0;
body: state + 1;
```

Z3 proof structure:
1. **Induction Base**: `requires(params) → invariant(params)`
2. **Preservation**: `invariant(params) ∧ requires(params) → invariant(body(params))`

This upgrades BMC's "no bugs in first N iterations" to "no bugs in **all** iterations".

### Call Graph Cycle Detection

Indirect recursion (A → B → A) is detected by DFS traversal of the call graph.
When a cycle is found, the verifier checks:
1. If `invariant:` is specified → inductive verification handles it (complete proof)
2. If `max_unroll:` is specified → BMC handles it (bounded proof)
3. Otherwise → warning emitted suggesting one of the above

### Taint Analysis

Values returned from `unverified` functions are marked with `__tainted_{call_id}`
in the Z3 environment. After body verification, `check_taint_propagation` scans
for tainted sources and warns if verification results depend on unverified code.

### Verification Steps (per atom)

0. `TrustLevel` check: skip/warn for trusted/unverified atoms
1. `verify_resource_hierarchy()`: Z3 checks priority ordering
1b. `verify_bmc_resource_safety()`: BMC for loop-internal acquire patterns (respects `max_unroll:`)
1c. `verify_async_recursion_depth()`: Recursive async call depth limit
1d. `verify_atom_invariant()`: Inductive invariant proof (base + preservation)
1e. `verify_call_graph_cycles()`: Indirect recursion detection via DFS
1f. `verify_effect_consistency()`: Effect inference + declared effects comparison (with subtyping)
1g. `verify_effect_params()`: Constant folding for literal paths + Z3 Int for variable paths
2. `expr_to_z3(Acquire)`: Tracks `__resource_held_{name}` as Z3 Bool
3. `expr_to_z3(Await)`: Resource-held-across-await + ownership consistency checks
4. Body verification + **taint analysis** (`check_taint_propagation`)
5. Standard `verify()` pipeline continues (requires/ensures/linearity)

### Pipeline Extension

```
source.mm → parse → resolve → monomorphize → verify (Z3) → codegen (LLVM IR) → transpile
                                                 ↑
                                    Resource Hierarchy Check
                                    Effect Inference & Verification
                                    Effect Hierarchy Resolution
                                    Deadlock-Free Proof
                                    Data Race Prevention
```

### LLVM IR Codegen

| Construct | LLVM IR Output |
|---|---|
| `acquire r { body }` | `call i32 @pthread_mutex_lock(@__mumei_resource_r)` → body → `call i32 @pthread_mutex_unlock(@__mumei_resource_r)` |
| `async { body }` | Synchronous compilation (future: `@llvm.coro.*` intrinsics) |
| `await expr` | Pass-through compilation (future: `@llvm.coro.suspend`) |

The `@__mumei_resource_{name}` global symbols are resolved at link time by the
Mumei runtime library or user-provided mutex instances. Since Z3 has proven the
acquisition order is deadlock-free, the runtime mutex only provides mutual
exclusion — not deadlock prevention.

### Transpiler Mapping (Async)

| Mumei | Rust | Go | TypeScript |
|---|---|---|---|
| `async atom f(x: T)` | `pub async fn f(x: T)` | `func f(x T) // goroutine` | `async function f(x: number)` |
| `await expr` | `expr.await` | `<-ch` | `await expr` |
| `acquire r { body }` | `let _g = r.lock(); { body }` | `r.Lock(); { body }; r.Unlock()` | `await r.acquire(); { body }; r.release()` |

---

## Effect System (Side-Effect Verification)

### Design

Mumei's Effect System allows developers to declare what side effects a function may
perform and uses Z3 to mathematically prove that no undeclared effects occur at
compile time. Atoms without an `effects:` annotation are treated as **Pure** (no
side effects allowed).

### Effect Definition

```mumei
// Basic effects
effect FileRead;
effect FileWrite;
effect Network;
effect Log;
effect Console;

// Composite effects (transitive inclusion)
effect IO includes: [FileRead, FileWrite, Console];
effect FullAccess includes: [IO, Network, Log];
```

Built-in effects are registered by `register_builtin_effects()` in `verification.rs`
and also defined in `std/effects.mm`.

### Atom Effect Annotation

```mumei
atom write_log(msg: i64)
effects: [Log, FileWrite];
requires: msg >= 0;
ensures: result >= 0;
body: {
    perform FileWrite.write(msg);
    perform Log.info(msg);
    msg
};

// Pure atom: no effects allowed
atom pure_add(a: i64, b: i64)
requires: true;
ensures: result == a + b;
body: a + b;
```

### Z3 Verification Model

The Effect System uses Z3 to prove the **effect containment** property:

```
ForAll e in UsedEffects(Body): e in AllowedEffects(Signature)
```

Z3 encoding:
1. For each known effect E, create a boolean `__effect_allowed_E`
2. Assert `__effect_allowed_E = true` for each E in the atom's declared effects (transitively expanded)
3. For each `perform E.op(...)` in the body, create `__effect_used_E = true`
4. Check that `(__effect_used_E AND NOT __effect_allowed_E)` is UNSAT for all E
5. If SAT, report the specific effect violation

### Effect Propagation

When atom A calls atom B, the verifier checks:

```
B.effects is subset of A.effects
```

If B requires effects not in A's allowed set, a propagation error is reported:

```
Effect propagation violation: atom 'log_only_caller' calls 'network_logger'
which requires [Network] effect, but 'log_only_caller' only declares [Log].
```

### EffectCtx

```rust
struct EffectCtx {
    allowed_effects: HashSet<String>,  // From atom's effects annotation (expanded)
    used_effects: HashSet<String>,     // From perform expressions in body
    violations: Vec<String>,           // Detected violations
}
```

### Verification Pipeline Integration

```
0. TrustLevel check
1. verify_resource_hierarchy()
1b-1e. (existing checks)
1f. verify_effect_containment() -- Z3 proof of effect set inclusion
2. expr_to_z3(Acquire): resource tracking
2b. expr_to_z3(Perform): effect usage tracking and verification
3-5. (existing pipeline continues)
```

### Code Generation

| Construct | LLVM IR Output |
|---|---|
| `perform Effect.op(args)` | `call i64 @__effect_Effect_op(args)` |

The `@__effect_{Effect}_{operation}` symbols are resolved at link time by the
runtime implementation of each effect.

### Transpiler Mapping (Effects)

| Mumei | Rust | Go | TypeScript |
|---|---|---|---|
| `effects: [FileWrite]` | `/// Effects: [FileWrite]` | `// Effects: [FileWrite]` | `@effects [FileWrite]` (JSDoc) |
| `perform FileWrite.write(x)` | `/* perform FileWrite.write */ filewrite_write(x)` | `/* perform FileWrite.write */ FileWriteWrite(x)` | `/* perform FileWrite.write */ filewritewrite(x)` |

### Error Reporting

Effect violations produce structured JSON in `report.json` for self-healing integration:

```json
{
    "status": "failed",
    "atom": "log_only",
    "violation_type": "effect_propagation_violation",
    "declared_effects": ["Log"],
    "required_effect": "Network",
    "suggested_fixes": ["Add [Network] to the effects declaration"],
    "reason": "Effect violation: atom 'log_only' declares [Log] but requires 'Network'"
}
```

### Standard Library

`std/effects.mm` defines the built-in effect hierarchy:
- **FileRead**, **FileWrite**, **Network**, **Log**, **Console** (basic effects)
- **IO** includes: FileRead, FileWrite, Console
- **FullAccess** includes: IO, Network, Log
