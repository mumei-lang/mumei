# 🔬 Mumei Language Reference
## Type System
### Refinement Types
Types with embedded logical predicates verified by Z3.
```mumei
type Nat = i64 where v >= 0;
type Pos = f64 where v > 0.0;
type NonZero = i64 where v != 0;
```
### Structs with Field Constraints
```mumei
struct Point {
    x: f64 where v >= 0.0,
    y: f64 where v >= 0.0
}
```
### Enums and Pattern Matching
```mumei
enum AtmState { Idle, Authenticated, Dispensing, Error }
atom classify_int(x)
    requires: true;
    ensures: result >= 0 && result <= 2;
    body: {
        match x {
            n if n > 0 => 0,
            0 => 1,
            _ => 2
        }
    }
```
Exhaustiveness checking uses SMT solving, not syntactic analysis.

### Enum Payloads (Tagged Unions)
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
## Atom Syntax
### Basic Atom
```mumei
atom name(param: Type, ...)
    requires: precondition;
    ensures: postcondition;
    body: expression;
```

### Explicit Return Type (`-> Type`)

By default, atoms return `i64`. Use `-> Type` to declare a different return type:

```mumei
// Returns Str (pointer type in LLVM IR)
atom greet(name: Str) -> Str
    requires: true;
    ensures: true;
    body: "Hello, " + name

// Returns f64
atom circle_area(r: f64) -> f64
    requires: r > 0.0;
    ensures: result > 0.0;
    body: r * r * 3.14159

// No annotation = defaults to i64
atom add(a: i64, b: i64)
    requires: true;
    ensures: result == a + b;
    body: a + b
```

Supported return types: `i64` (default), `f64`, `Str`, `[i64]`, enum types.

### Str Type

First-class string type. String literals are `Str` values. Supported operations:

| Operation | Syntax | Result Type |
|---|---|---|
| Concatenation | `a + b` | `Str` |
| Equality | `a == b` | `i64` (0 or 1) |
| Inequality | `a != b` | `i64` (0 or 1) |

```mumei
atom is_same(a: Str, b: Str)
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: { if a == b { 1 } else { 0 } }
```

---
## Generics and Trait Bounds
### Generics (Monomorphization)
```mumei
struct Pair<T, U> { first: T, second: U }
enum Option<T> { Some(T), None }
atom identity<T>(x: T) requires: true; ensures: true; body: x;
```
### Trait Definitions with Laws
```mumei
trait Comparable {
    fn leq(a: Self, b: Self) -> bool;
    law reflexive: leq(x, x) == true;
    law transitive: leq(a, b) && leq(b, c) => leq(a, c);
}
impl Comparable for i64 {
    fn leq(a: i64, b: i64) -> bool { a <= b }
}
```
### Trait Method Refinement Constraints
```mumei
trait Numeric {
    fn add(a: Self, b: Self) -> Self;
    fn div(a: Self, b: Self where v != 0) -> Self;
    law commutative_add: add(a, b) == add(b, a);
}
```
### Built-in Traits
| Trait | Methods | Laws |
|---|---|---|
| **Eq** | `eq(a, b) -> bool` | reflexive, symmetric |
| **Ord** | `leq(a, b) -> bool` | reflexive, transitive |
| **Numeric** | `add`, `sub`, `mul`, `div(b where v!=0)` | commutative_add |
---
## Termination Checking
1. **Bounded below**: `invariant && cond => V >= 0`
2. **Strict decrease**: After each iteration, `V' < V`
```mumei
while i < n
invariant: s >= 0 && i <= n
decreases: n - i
{
    s = s + i;
    i = i + 1;
};
```
---
## Module System
### Import Syntax
```mumei
import "std/option" as option;
import "./lib/math.mm" as math;
```
### Inter-atom Function Calls (Compositional Verification)
1. Caller proves `requires` at the call site
2. Caller assumes `ensures` as a fact
3. Body is NOT re-verified
```mumei
atom increment(n: Nat)
requires: n >= 0;
ensures: result >= 1;
body: { n + 1 };
atom double_increment(n: Nat)
requires: n >= 0;
ensures: result >= 2;
body: {
    let x = increment(n);
    increment(x)
};
```
---
## Quantifiers in Contracts
```mumei
trusted atom verified_insertion_sort(n: i64)
requires: n >= 0;
ensures: result == n && forall(i, 0, result - 1, arr[i] <= arr[i + 1]);
body: n;
atom binary_search_sorted(n: i64, target: i64)
requires: n >= 0 && forall(i, 0, n, arr[i] <= arr[i + 1]);
ensures: result >= 0 - 1 && result < n;
body: { ... };
```
---
## Ownership and Borrowing
| Modifier | Semantics | Z3 Tracking |
|---|---|---|
| (none) | Owned value | `__alive_` Bool |
| `ref` | Shared read-only | `__borrowed_` Bool |
| `ref mut` | Exclusive mutable | `__exclusive_` Bool |
| `consume` | Ownership transfer | `__alive_` set to false |
---
## Async/Await and Resource Hierarchy
```mumei
resource db_conn priority: 1 mode: exclusive;
resource cache   priority: 2 mode: shared;
async atom transfer(amount: i64)
resources: [db_conn, cache];
requires: amount >= 0;
ensures: result >= 0;
body: {
    acquire db_conn { acquire cache { amount } }
};
```
---
## Higher-Order Functions (Phase A)

### atom_ref — First-Class Function References

Reference an atom as a value using `atom_ref(name)`. Invoke via `call(f, args...)`.

```mumei
atom increment(x: i64)
    requires: x >= 0;
    ensures: result == x + 1;
    body: x + 1;

// Direct atom_ref: contract IS propagated
atom demo()
    requires: true;
    ensures: result >= 0;
    body: call(atom_ref(increment), 5);
```

### Function Type Parameters

Atoms can accept function-type parameters using `atom_ref(T) -> R` syntax:

```mumei
trusted atom apply(x: i64, f: atom_ref(i64) -> i64)
    requires: x >= 0;
    ensures: result >= 0;
    body: call(f, x);

// At call site, concrete atom's contract is propagated:
atom demo_apply()
    requires: true;
    ensures: result >= 0;
    body: apply(5, atom_ref(increment));
```

> **Phase A limitation**: When `call(f, x)` uses a function-type *parameter* `f` (not a literal `atom_ref(name)`), the verifier returns an unconstrained symbolic value. Such atoms must be marked `trusted`. Phase B will resolve this with `call_with_contract`.

---
## FFI (Foreign Function Interface)

### extern Blocks

Declare external functions implemented in Rust or C:

```mumei
extern "Rust" {
    fn json_parse(input: Str) -> i64;
    fn json_stringify(handle: i64) -> Str;
    fn json_free(handle: i64) -> i64;
}
```

Extern functions are auto-registered as `trusted` atoms. The `return_type` annotation is propagated to LLVM codegen via `resolve_return_type()`.

### Memory Management

FFI handle stores (JSON, HTTP, String) support explicit deallocation:

```mumei
import "std/json" as json;

atom process_json(input: Str)
    requires: true;
    ensures: result >= 0;
    body: {
        let obj = json::parse(input);
        let value = json::get_int(obj, "key");
        let _ = json::free(obj);
        value
    }
```

| Function | Description |
|---|---|
| `json::free(handle)` | Release JSON handle (1=success, 0=invalid) |
| `json::str_free(handle)` | Release string handle |
| `http::free(handle)` | Release HTTP response handle |

---
## Stateful Effects (Temporal Ordering)

Effects can define states and transitions verified at compile time:

```mumei
effect File
    states: [Closed, Open];
    initial: Closed;
    transition open: Closed -> Open;
    transition write: Open -> Open;
    transition close: Open -> Closed;
```

The compiler uses forward dataflow analysis on the MIR CFG to verify operations occur in valid states. Conflicting states at merge points (e.g., `if/else` leaving a file in different states) are checked via Z3 Int Sort probes.

---
## Trust Boundary
```mumei
trusted atom ffi_read(fd: i64)
requires: fd >= 0;
ensures: result >= 0;
body: fd;
unverified atom legacy_code(x: i64)
requires: x >= 0;
ensures: result >= 0;
body: x + 1;
```
