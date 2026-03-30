# Verification Patterns

> Practical patterns for leveraging Mumei's formal verification capabilities in real-world applications.

## 1. Verified Configuration

### Motivation

Configuration errors are a leading cause of production incidents. Mumei's refinement types let you encode configuration constraints directly in the type system, so Z3 proves at compile time that all configuration values are within valid ranges.

### Syntax

```mumei
// Define constrained types for each config parameter
type Port = i64 where v >= 1 && v <= 65535;
type Timeout = i64 where v >= 100 && v <= 30000;
type MaxRetries = i64 where v >= 1 && v <= 10;

// Atoms using these types get automatic constraint verification
atom validate_server_config(port: Port, timeout: Timeout, retries: MaxRetries)
    requires: port >= 1 && port <= 65535 && timeout >= 100 && retries >= 1;
    ensures: result == 1;
    body: 1;
```

### Example

See [`examples/verified_config.mm`](../examples/verified_config.mm) for a complete example.

Key features:
- **Refinement types** encode valid ranges at the type level
- **Preconditions** (`requires`) specify additional constraints
- **Postconditions** (`ensures`) guarantee return value properties
- Z3 proves that the body satisfies all contracts at compile time

### How Z3 Verifies It

1. When `validate_server_config` is called, Z3 checks that the caller provides values satisfying the refinement type predicates (`v >= 1 && v <= 65535` for Port, etc.)
2. The `requires` clause is asserted as a precondition
3. Z3 proves that the body expression (`1`) satisfies the `ensures` clause (`result == 1`)
4. If any constraint is unsatisfiable, Z3 returns a counter-example showing the violating input values

---

## 2. Verified State Machine

### Motivation

Business processes often follow strict state transition rules (e.g., an order must be paid before it can be shipped). Mumei's temporal effect verification models these as state machines, and Z3 proves at compile time that all transitions follow the declared protocol.

### Syntax

```mumei
// Declare a stateful effect with explicit state machine
effect Order
    states: [Created, Paid, Shipped, Delivered, Cancelled];
    initial: Created;
    transition pay: Created -> Paid;
    transition ship: Paid -> Shipped;
    transition deliver: Shipped -> Delivered;
    transition cancel: Created -> Cancelled;

// Atoms declare their effect pre/post states
atom process_order(x: i64)
    effects: [Order];
    effect_pre: { Order: Created };
    effect_post: { Order: Shipped };
    requires: x > 0;
    ensures: result >= 0;
    body: {
        perform Order.pay;    // Created -> Paid
        perform Order.ship;   // Paid -> Shipped
        x
    }
```

### Example

See [`examples/order_state_machine.mm`](../examples/order_state_machine.mm) for valid transitions and [`examples/order_state_machine_error.mm`](../examples/order_state_machine_error.mm) for an invalid transition that Z3 catches.

Key features:
- **Stateful effects** model business process states
- **`effect_pre`/`effect_post`** declare the expected state at entry and exit
- **`perform`** triggers state transitions
- Invalid transitions (e.g., shipping before payment) are caught at compile time

### How Z3 Verifies It

1. The temporal effect verifier tracks the current state through the atom's body using forward dataflow analysis
2. Each `perform Order.pay` is checked against the state machine: is `pay` valid from the current state (`Created`)? Yes — transition to `Paid`
3. Each `perform Order.ship` is checked: is `ship` valid from `Paid`? Yes — transition to `Shipped`
4. At atom exit, the final state (`Shipped`) is checked against `effect_post` (`{ Order: Shipped }`) — match confirmed
5. If the code attempts `perform Order.ship` from state `Created` (skipping payment), the verifier reports an `InvalidPreState` error with the expected state (`Paid`) and actual state (`Created`)

---

## Related Documents

- [Language Reference](LANGUAGE.md) — Full syntax documentation
- [Architecture](ARCHITECTURE.md) — Compiler internals and temporal effect verification
- [Roadmap](ROADMAP.md) — Strategic roadmap including verification patterns
