# 📄 Mumei Examples & Test Suite Reference
## Verification Suite (`sword_test.mm`)
The test suite exercises **8 atoms**, **2 structs**, **1 generic struct**, **1 generic enum**, **1 trait + impl**, covering every verification feature:
```mumei
type Nat = i64 where v >= 0;
type Pos = f64 where v > 0.0;
struct Point { x: f64 where v >= 0.0, y: f64 where v >= 0.0 }
struct Pair<T, U> { first: T, second: U }
enum Option<T> { Some(T), None }
trait Comparable {
    fn leq(a: Self, b: Self) -> bool;
    law reflexive: leq(x, x) == true;
}
impl Comparable for i64 {
    fn leq(a: i64, b: i64) -> bool { a <= b }
}
atom sword_sum(n: Nat) ...   // Loop invariant + termination
atom scale(x: Pos) ...       // Float refinement
atom stack_push(...) ...      // Overflow prevention
atom stack_pop(...) ...       // Underflow prevention
atom circle_area(r: Pos) ... // Geometric invariant
atom robust_push(...) ...     // Bounded stack push
atom stack_clear(...) ...     // Termination proof
atom dist_squared(...) ...    // Non-negative guarantee
```
### Verified Properties
| Atom | Verification |
|---|---|
| `sword_sum` | Loop invariant + **termination** (`decreases: n - i`) |
| `scale` | Float refinement (Pos > 0.0 ⟹ result > 0.0) |
| `stack_push` | Overflow prevention (top < max ⟹ top+1 ≤ max) |
| `stack_pop` | Underflow prevention (top > 0 ⟹ top-1 ≥ 0) |
| `circle_area` | Geometric invariant (r > 0 ⟹ area > 0) |
| `robust_push` | Bounded stack push (0 ≤ top' ≤ max) |
| `stack_clear` | Loop **termination** (`decreases: i`) + invariant preservation |
| `dist_squared` | Non-negative distance (dx² + dy² ≥ 0) |
| `Pair<T,U>` | Generic struct (monomorphization) |
| `Option<T>` | Generic enum (monomorphization) |
| `Comparable` | Trait law `reflexive` verified by Z3 for `impl i64` |
---
## Pattern Matching Test (`examples/match_atm.mm`)
Demonstrates Enum + match + guards + Refinement Types:
```mumei
type Balance = i64 where v >= 0;
enum AtmState { Idle, Authenticated, Dispensing, Error }
atom atm_transition(state, action, balance: Balance)
    requires: state >= 0 && state <= 3 && action >= 0 && action <= 3;
    ensures: result >= 0 && result <= 3;
    body: {
        match state {
            0 => match action { 0 => 1, _ => 3 },
            1 => match action { 1 => 2, 3 => 0, _ => 3 },
            2 => match action { 2 if balance > 0 => 0, 2 => 3, 3 => 0, _ => 3 },
            _ => 3
        }
    }
```
### Transpiler Output
**Rust:**
```rust
pub enum AtmState { Idle, Authenticated, Dispensing, Error }
```
**Go:**
```go
type AtmState int64
const ( Idle AtmState = iota; Authenticated; Dispensing; Error )
```
**TypeScript:**
```typescript
export const enum AtmStateTag { Idle, Authenticated, Dispensing, Error }
```
---
## Inter-atom Call Test (`examples/call_test.mm`)
```mumei
atom increment(n: Nat) requires: n >= 0; ensures: result >= 1; body: { n + 1 };
atom double_increment(n: Nat) requires: n >= 0; ensures: result >= 1;
body: { let x = increment(n); increment(x) };
```
---
## Multi-file Import Test (`examples/import_test/`)
```
examples/import_test/
├── lib/math_utils.mm    # safe_add, safe_double
└── main.mm              # import "./lib/math_utils.mm" as math;
```
---
## Higher-Order Functions Demo (`examples/higher_order_demo.mm`)

Demonstrates `atom_ref` + `call` + `contract()` for first-class function references with Z3-verified contracts:

```mumei
atom increment(x: i64)
    requires: x >= 0;
    ensures: result == x + 1;
    body: x + 1;

// contract(f) lets Z3 verify without trusted (Phase B: call_with_contract)
atom apply(x: i64, f: atom_ref(i64) -> i64)
    requires: x >= 0;
    ensures: result >= 0;
    contract(f): ensures: result >= 0;
    body: call(f, x);

// At call site, increment's contract IS propagated via atom_ref
atom demo_apply()
    requires: true;
    ensures: result >= 0;
    body: apply(5, atom_ref(increment));
```

```bash
mumei verify examples/higher_order_demo.mm   # Z3 verification
mumei build examples/higher_order_demo.mm -o dist/higher_order_demo
```

---
## Str Type Demo (`examples/str_demo.mm`)

Demonstrates `Str` type string operations with `-> Str` return type annotation (Plan 9 + Plan 18):

```mumei
atom greet(name: Str) -> Str
    requires: true;
    ensures: true;
    body: "Hello, " + name

atom is_same(a: Str, b: Str)
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: { if a == b { 1 } else { 0 } }
```

---
## Enum Payload Demo (`examples/enum_payload.mm`)

Demonstrates tagged unions with payload data and `match` pattern matching (Plan 14):

```mumei
enum Shape {
    Circle(i64),
    Rectangle(i64, i64)
}

atom area(s: Shape)
    requires: true;
    ensures: result >= 0;
    body: {
        match s {
            Circle(r) => r * r * 3,
            Rectangle(w, h) => w * h
        }
    }
```

---
## JSON Demo (`examples/json_demo.mm`)

Demonstrates `std.json` operations — object construction, array manipulation, and stringify (Plan 10 + Plan 17):

```mumei
import "std/json" as json;

atom build_user(name: Str, age: i64)
    requires: age >= 0;
    ensures: result >= 0;
    body: {
        let obj = json::object_new();
        let name_val = json::from_str(name);
        let age_val = json::from_int(age);
        let obj = json::object_set(obj, "name", name_val);
        let obj = json::object_set(obj, "age", age_val);
        obj
    }
```

---
## HTTP Demo (`examples/http_demo.mm`)

Demonstrates `std.http` GET requests and response processing (Plan 11 + Plan 17):

```mumei
import "std/http" as http;

atom fetch_status(url: Str)
    requires: true;
    ensures: result >= 0;
    body: {
        let response = http::get(url);
        http::status(response)
    }
```

---
## Concurrent HTTP Demo (`examples/concurrent_http.mm`)

Demonstrates `task_group` for parallel HTTP requests (Plan 8 + Plan 11):

```mumei
import "std/http" as http;

atom fetch_all(url1: Str, url2: Str)
    requires: true;
    ensures: result >= 0;
    body: {
        task_group(all) {
            task { fetch_one(url1) },
            task { fetch_one(url2) }
        }
    }
```

---
## E2E Verification Tests

| File | Tests |
|---|---|
| `tests/test_str_type.mm` | Str concat, equality, inequality, empty string |
| `tests/test_enum_payload.mm` | Match variants, wildcards, nested match |
| `tests/test_json_operations.mm` | Object roundtrip, array ops, type checks |

```bash
for f in tests/test_*.mm; do
    mumei check "$f" && echo "PASS ✓" || echo "FAIL ✗"
done
```

---
## Negative Test Suite
| File | Expected Error | Category |
|---|---|---|
| `postcondition_fail.mm` | Postcondition not satisfied | Basic |
| `division_by_zero.mm` | Potential division by zero | Safety |
| `array_oob.mm` | Potential Out-of-Bounds | Safety |
| `match_non_exhaustive.mm` | Match is not exhaustive | Completeness |
| `consume_ref_conflict.mm` | Cannot consume ref parameter | Ownership |
| `invariant_fail.mm` | Invariant fails initially | Loop |
| `requires_not_met.mm` | Precondition not satisfied at call site | Inter-atom |
| `termination_fail.mm` | Decreases does not strictly decrease | Termination |
| `forall_ensures_fail.mm` | forall in ensures not satisfied | Quantifier |
```bash
for f in tests/negative/*.mm; do
    mumei verify "$f" && echo "UNEXPECTED PASS" || echo "EXPECTED FAIL ✓"
done
```
---
## Outputs
| Output | Path | Contents |
|---|---|---|
| LLVM IR | `dist/katana_<AtomName>.ll` | Pattern Matrix match, StructType |
| Rust | `dist/katana.rs` | `enum` + `struct` + `fn` with `match` |
| Go | `dist/katana.go` | `const+type` + `struct` + `func` with `switch` |
| TypeScript | `dist/katana.ts` | `const enum` + `interface` + `function` |
