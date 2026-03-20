# 🏗️ Mumei Compiler Architecture

## Pipeline

```
source.mm → parse → resolve → monomorphize → lower_to_hir → verify (Z3) → codegen (LLVM IR) → transpile (Rust/Go/TS)
                                                    ↑                          ↑
                                      HIR: Expr/Stmt separation         lower_to_mir (Phase 4b)
                                      Resource Hierarchy Check           CFG-based MIR for
                                      Effect Containment Check           borrow checking (future)
                                      Async Safety Verification
                                      LinearityCtx wiring (Phase 4a)
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
| `src/parser/item.rs` | Top-level item parsing — fully migrated to recursive descent (no regex), `contract()` clause parsing for higher-order function parameters |
| `src/mir.rs` | MIR (Mid-level IR) definitions — CFG-based BasicBlocks, three-address code, basic HIR → MIR lowering |
| `src/mir_analysis.rs` | MIR dataflow analyses — liveness (backward), drop insertion, move analysis (forward), ConflictingMerge detection |
| `src/parser/pattern.rs` | Pattern parsing for match arms |
| `src/ast.rs` | `TypeRef`, `Monomorphizer` — generic type expansion engine |
| `src/resolver.rs` | Import resolution, circular detection, prelude auto-load, incremental build cache |
| `src/verification.rs` | Z3 verification, `ModuleEnv`, `LinearityCtx`, law expansion, equality propagation, resource hierarchy, BMC, async recursion depth, inductive invariant, trust boundary, `call_with_contract` (Phase B higher-order function verification) |
| `src/codegen.rs` | LLVM IR generation — `resolve_return_type()`, Pattern Matrix, StructType, malloc/free, nested extract_value |
| `src/hir.rs` | HIR (High-level IR) definitions, AST → HIR lowering, `HirEffectSet` on `HirAtom`/`HirExpr::Call`/`HirExpr::Perform` |
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
    pub security_policy: Option<SecurityPolicy>,            // Effect parameter constraints
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

0. Trust level check (trusted → skip, unverified → warn)
1a. Resource hierarchy verification (deadlock prevention)
1b. BMC resource safety (loop-internal acquire patterns)
1c. Async recursion depth check
1d. Atom invariant induction (base + preservation)
1e. Call graph cycle detection (indirect recursion)
1f. Effect containment verification
1g. Effect parameter constraint verification (constant folding)
1h. MIR-based move analysis (UseAfterMove, DoubleMove, ConflictingMerge → warnings)
1i. Temporal effect verification (stateful effects — forward dataflow state tracking + Z3 Int Sort conflict probes)
2. Quantifier constraints (`forall`/`exists`)
2b. Refinement type injection (params → Z3 symbolic variables)
2c. Struct field constraints (recursive for nested structs)
2d. Array length symbols (`len_<name> >= 0`)
2e. Linearity setup (`__alive_`/`__borrowed_` Z3 Bools)
2f. Effect allowed set injection (`__effect_allowed_*` Z3 Bools)
3. `requires` assertion + aliasing prevention
4. Body evaluation (`stmt_to_z3` / `expr_to_z3` — incl. Z3 String Sort for effect params)
5. `ensures` verification (negate + check Sat)
5b. Linearity finalization (consume marking + violation check)
6. Contradiction check (unsat core analysis)

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
1f. `verify_effect_containment()`: Z3 proof of effect set inclusion
1f. `verify_effect_consistency()`: Effect inference + declared effects comparison (warning level)
1g. `verify_effect_params()`: Constant folding for literal paths + Z3 Int for variable paths
2. `expr_to_z3(Acquire)`: Tracks `__resource_held_{name}` as Z3 Bool
2b. `expr_to_z3(Perform)`: `EffectCtx` tracks usage + `SecurityPolicy` enforcement + Z3 containment check
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

### Effect Polymorphism

Mumei supports effect polymorphism through `<E: Effect>` type parameter bounds,
resolved via monomorphization (same mechanism as type polymorphism).

#### Syntax

```mumei
atom pipe<E: Effect>(f: atom_ref(i64) -> i64 with E)
    effects: [E];
    requires: true;
    ensures: true;
    body: call(f, 42);
```

#### Resolution Flow

```
1. Parser: atom pipe<E: Effect>(...) → type_params=["E"], where_bounds=[{E: Effect}]
2. Monomorphizer: pipe<FileWrite>(...) → type_map={E: FileWrite}
   - effects: [E] → effects: [FileWrite]
   - with E → with [FileWrite]
   - perform E.op → perform FileWrite.op
3. Verifier: verify pipe<FileWrite> as a concrete atom
   - verify_effect_containment: FileWrite ⊆ caller's effects
   - Z3: standard effect containment proof
```

#### Key Design Decision

Effect polymorphism is resolved at compile time through monomorphization, not through
Z3 quantifiers. This ensures:
- No runtime overhead (zero-cost abstraction)
- Predictable verification performance (no universal quantification)
- Consistent with mumei's existing generics system

### Z3 String Sort for Effect Parameters

Effect parameters (e.g., `FileRead(path: Str)`) support two verification strategies:

**Constant Folding** (Rust-side):
When the argument is a literal (e.g., `perform FileRead.read("/tmp/data.txt")`),
`check_constant_constraint()` evaluates the constraint directly in Rust with zero Z3 overhead.

**Z3 String Sort** (Symbolic):
When the argument is a variable (e.g., `perform FileRead.read(path)`),
the verifier creates a `z3::ast::String` variable and asserts the effect's constraint
as a Z3 String operation:

| Constraint | Z3 Operation |
|---|---|
| `starts_with(path, "/tmp/")` | `Z3String::prefix_of(prefix, path_z3)` |
| `ends_with(path, ".txt")` | `Z3String::suffix_of(suffix, path_z3)` |
| `contains(path, "data")` | `Z3String::contains(path_z3, substr)` |
| `not_contains(path, "..")` | `NOT Z3String::contains(path_z3, substr)` |

The `parse_constraint_to_z3_string()` function in `verification.rs` handles this mapping.

**Sort-aware Timeout**: When String Sort constraints are detected (via pre-scan of
effect definitions), the Z3 solver timeout is automatically doubled because String
Sort solving is significantly slower than Int Sort.

**Constraint Budget**: Each Z3 String constraint creation is tracked against the
per-atom constraint budget (default: 1000) to prevent solver explosion.

### Stateful Effects (Temporal Effect Verification)

Mumei supports **stateful effects** — effects with defined states and transitions that
are verified at compile time. This enables temporal ordering verification (e.g., a file
must be opened before writing, and closed after use).

#### Syntax

```mumei
effect File
    states: [Closed, Open];
    initial: Closed;
    transition open: Closed -> Open;
    transition write: Open -> Open;
    transition read: Open -> Open;
    transition close: Open -> Closed;
```

#### Verification (Phase 1i)

The temporal effect verifier uses **forward dataflow analysis** on the MIR CFG:

1. **EffectStateMachine**: Built from `EffectDef` with states, transitions, and initial state
2. **Forward Dataflow**: Worklist algorithm tracks effect state at each basic block entry/exit
3. **Violation Detection**:
   - `InvalidPreState`: Operation performed in wrong state (e.g., write when Closed)
   - `ConflictingState`: Merge point has conflicting states from different branches (Z3 Int Sort probe: UNSAT → hard error, SAT → info, Unknown → warning)
   - `UnexpectedFinalState`: Function exits with effect in unexpected state

**Explosion Prevention**:
- State machine size limited to 8 states (`MAX_EFFECT_STATES`)
- Iteration limit: `block_count * max(state_machines_count, 10)`
- Abstract interpretation (Rust-side) before Z3 — Z3 reserved for ConflictingState cases
- Per-atom constraint budget applies to temporal Z3 constraints

#### Modular Verification (Implemented)

Atom-level `effect_pre` / `effect_post` contracts enable verifying atoms independently:
- Each atom declares required pre-state and guaranteed post-state for each stateful effect
- Callers verify against the callee's contract without analyzing the callee's body
- `effect_pre` overrides the initial state of corresponding state machines during verification
- `effect_post` is checked against the exit states of the atom's body; mismatch emits `UnexpectedFinalState`
- Syntax: `effect_pre: { File: Closed }; effect_post: { File: Open };`
- Default: empty `HashMap` (backward compatible with all existing atoms)
- **Validation**: Invalid state names produce hard errors; missing state machines emit warnings
- **Monomorphization**: Effect type variables in keys are substituted (e.g., `{ E: Closed }` → `{ FileWrite: Closed }`)
- **Limitation**: Cross-atom contract composition at call sites is not yet implemented — each atom is verified independently

### Standard Library

`std/effects.mm` defines the built-in effect hierarchy:
- **FileRead**, **FileWrite**, **Network**, **Log**, **Console** (basic effects)
- **IO** includes: FileRead, FileWrite, Console
- **FullAccess** includes: IO, Network, Log
