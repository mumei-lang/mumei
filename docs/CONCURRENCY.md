# Structured Concurrency Design Document

> Mumei's structured concurrency and Z3 verification strategy.

## Overview

Mumei adopts **Structured Concurrency**, formally guaranteeing task lifecycle
properties through the type system and Z3 solver.
By verifying at compile time that parent tasks do not terminate before child tasks,
dangling tasks and resource leaks are prevented.

## Existing Async Foundation

### async atom

```mumei
async atom fetch_data(url: String) -> Result<String, Error>
    requires: url.len() > 0;
    ensures: result.is_ok();
    body: ...;
```

### acquire / await

```mumei
acquire db_conn {
    let data = await fetch_data("https://...");
    process(data)
}
```

### Resource Definitions

```mumei
resource db_conn priority: 1 mode: exclusive;
resource cache   priority: 2 mode: shared;
```

## Task Syntax

### task Expression

Spawns a child task. Executes within the parent task's scope,
with structured concurrency guaranteeing the parent does not terminate first.

```mumei
task {
    // child task body
    compute_heavy_work(data)
}

// specify group name
task workers {
    process_item(item)
}
```

### AST Representation

```rust
Expr::Task {
    body: Box<Expr>,
    group: Option<String>,  // task group name (default if omitted)
}
```

## TaskGroup Syntax

### task_group Expression

Groups multiple child tasks and waits for completion according to Join semantics.

```mumei
// Wait for all tasks to complete (default: All)
task_group {
    task { fetch_users() };
    task { fetch_orders() };
    task { fetch_products() }
}

// Continue on first completion (Any)
task_group:any {
    task { primary_server() };
    task { fallback_server() }
}
```

### AST Representation

```rust
Expr::TaskGroup {
    children: Vec<Expr>,
    join_semantics: JoinSemantics,
}

pub enum JoinSemantics {
    All,  // Wait for all tasks to complete (default)
    Any,  // Return the result of the first completed task
}
```

## Z3 Verification Strategy

### Structured Concurrency Guarantees

The Z3 solver verifies the following safety properties at compile time:

#### 1. Parent Task Termination Constraint

**Constraint**: Parent task must not terminate before child tasks.

```
JoinSemantics::All:
  parent_done => ∀i. child_done[i]
  (parent completion requires all child tasks to complete)

JoinSemantics::Any:
  parent_done => ∃i. child_done[i]
  (parent completion requires at least one child task to complete)
```

#### 2. Resource Hold Verification (existing)

Verifies that resources are not held across `await` points:

```
await inside acquire block → deadlock risk → error
```

#### 3. Ownership Consistency (existing)

Verifies that consumed variables before `await` are not accessed after `await`.

#### 4. Structured Concurrency Ownership (Phase 1h-2)

MIR lowering flattens a `task_group` into a *sequential* chain of child bodies,
so MIR move analysis models neither concurrent interleaving of siblings nor the
cancellation of losing children in `task_group:any`. An AST-level pass
(`mumei-core/src/verification/support/task_ownership.rs`, run as verification
phase `Phase 1h-2`) closes that gap and rejects:

| Violation | Pattern |
|---|---|
| `ConcurrentDoubleMove` | the same captured value is consumed by two sibling tasks |
| `MoveWhileSiblingUses` | one sibling consumes a capture a concurrent sibling still reads |
| `ConcurrentDataRace` | a capture is written by one sibling and read/written by another |
| `UseAfterConcurrentMove` | the parent uses a value a child task consumed |
| `CancelDependentRead` | the parent reads a value written by a `task_group:any` child, whose write a cancelled child may never have performed |

Captures of any type are covered (arrays, structs, pointers, `f64`), because
movability is derived from the declared type via `mir::movability_from_type`
rather than from the i64 marshalling path in codegen. Shared *reads* across
siblings and writes to task-local bindings remain legal. (Codegen-side array
element capture in task bodies is still a follow-up; the ownership obligation is
checked regardless of whether codegen can lower the capture.) Struct captures use
the `struct Name { field: T }` declaration form — `type Name = ...` declares a
refinement type over a base type, not an aggregate.

Rejections are also machine-readable: `mumei verify --json` lists one
`diagnostics` entry per rejected atom (`code: "failed"`, `severity: "error"`,
plus the rejection message). See [REPORT_SCHEMA.md](REPORT_SCHEMA.md).

These obligations are decided syntactically: they always produce a hard
verification error and never a Z3 `unknown`, so they never enter the Lean
escalation path (`lean_solver_time_s`, `MumeiLean/Ownership.lean`) and can
never be promoted to `lean_verified`.

#### 5. Cancellation Resource Release

For both join semantics, the group only completes once every child released the
resources it acquired: `parent_done ⇒ resource_released` for each `acquire` in a
child, alongside the existing `cancelled ⇒ resource_released`. Under
`JoinSemantics::All` no child is cancelled, which is asserted as
`parent_done ⇒ ¬cancelled_i`.

### Verification Flow

```
1. Parse task { body }
2. Recursively verify body safety with Z3
3. Verify each child task within TaskGroup
4. Assert termination constraints to Z3 solver based on Join semantics
5. Check constraint satisfaction → compile error on violation
```

## Implementation Status

| Item | Status |
|---|---|
| `Expr::Task` / `Expr::TaskGroup` AST | ✅ Implemented |
| `JoinSemantics` enum (All/Any) | ✅ Implemented |
| `task` / `task_group` parsing | ✅ Implemented (`:all` / `:any` support, invalid token detection) |
| Z3 join constraints (symbolic Bool) | ✅ Implemented (parent_done ⇒ child_done) |
| Full AST walker support | ✅ Implemented (collect_callees, count_self_calls, collect_acquire_resources, collect_from_expr) |
| LLVM codegen — `task` / `task_group` | ✅ Implemented (Plan 21: each `task` lowers to a `__mumei_task_<atom>_<N>` wrapper invoked via `pthread_create` + `pthread_join`; i64 captures marshalled through a stack-allocated args struct; result read back from the struct's tail slot. See [`compile_task_spawn`](../mumei-emit-llvm/src/codegen.rs).) |
| LLVM codegen — `chan` send/recv | ✅ Implemented (Plan 21: `send` / `recv` lower to `__mumei_chan_send` / `__mumei_chan_recv` runtime calls, backed by `pthread_mutex_t` + `pthread_cond_t` in [`runtime/mumei_runtime.c`](../runtime/mumei_runtime.c).) |
| Parser tests | ✅ Implemented (6 tests: task, task_group, :all, :any, unknown panic) |
| Unique ID (Task) | ✅ Implemented (TASK_COUNTER prevents env key collision) |
| Runtime scheduler | ✅ Implemented (Plan 21: pthread-backed; one OS thread per `task`; channel rendezvous via single-slot mutex/condvar in `mumei_runtime.c`) |
| Task cancellation | ✅ Implemented (`task_group:any` winners atomically cancel remaining children; blocked channels are woken via runtime broadcasts) |
| Concurrent capture ownership (`task_group:all` / `:any`) | ✅ Implemented (Phase 1h-2: concurrent double move, move racing a sibling read, unsynchronised shared writes, parent use after a child's move, and cancellation-dependent reads are hard errors) |
| Non-i64 captures in ownership checks | ✅ Implemented (Phase 1h-2 derives movability from the declared type, so array / struct / `f64` / pointer captures are checked; codegen still marshals only scalar and aggregate-by-value captures — array *element storage* capture in task bodies remains a codegen follow-up in `mumei-emit-llvm/src/codegen/task_runtime.rs`) |
| Channel types | ✅ Implemented (Plan 21: i64 handles + runtime mutex/condvar; full polymorphic `chan<T>` payload-marshalling is a follow-up) |
| `task_group:any` (atomic completion flag) | ✅ Implemented (first child to complete wins via `__mumei_task_group_complete`; remaining children are cancelled, woken, and joined for cleanup) |

## Safety Guarantees

| Property | Verification Method | Status |
|---|---|---|
| Deadlock prevention | Z3 verification of resource hierarchy (priority) | ✅ Implemented |
| Resource hold across await | Detect await inside acquire block | ✅ Implemented |
| Async recursion depth | BMC unroll limit check | ✅ Implemented |
| Parent task termination constraint | Z3 verification of TaskGroup join semantics | ✅ Implemented |
| Task cancellation safety | Atomic `task_group:any` completion, cooperative cancellation checks, channel wakeup broadcasts, and final `pthread_join` cleanup | ✅ Implemented |
| Data-race freedom on captured variables | AST-level sibling read/write analysis over `task_group` children (Phase 1h-2) | ✅ Implemented |
| Concurrent ownership (double move / move racing a sibling use) | AST-level move analysis over `task_group` children, movability from declared types | ✅ Implemented |
| Ownership consistency under cancellation | Parent reads of `task_group:any` child writes rejected; Z3 `parent_done ⇒ resource_released` for every child `acquire` | ✅ Implemented |

## Future Extensions

> Details: [`docs/ROADMAP.md`](ROADMAP.md)

### Roadmap P1-D: std.http Integration

Integration demo with `task_group:all` + parallel HTTP requests is planned in P1-D:

```mumei
import "std/http" as http;

// Concurrent API requests — practical task_group usage
task_group:all {
    task { http.get("https://api.example.com/users") };
    task { http.get("https://api.example.com/orders") };
    task { http.get("https://api.example.com/products") }
}
```

### Concurrency Refinements

1. **Runtime scheduler**: Preemptive task scheduling
2. **Channel types**: Type-safe channels for inter-task communication (`chan<T>`)
3. **Task cancellation refinements**: timeout/deadline policies and richer cancellation diagnostics (ownership consistency under cancellation is implemented; see Safety Guarantees)
4. **Timeouts**: Timeout specification for task groups
5. **LLVM codegen**: LLVM coroutine transformation for task scheduling code
6. **TaskGroup unique ID**: Prevent Z3 variable name collision across multiple TaskGroups (TASK_GROUP_COUNTER)
7. **Return type inference**: Auto-infer return type from Task body
8. **Result binding syntax**: Syntax to bind `task_group` results to variables

## Related Files

- `mumei-core/src/parser/` — `Task`, `TaskGroup`, `JoinSemantics` definitions + parsing + tests
- `mumei-core/src/verification/translator/stmt.rs` — Z3 structured concurrency verification (symbolic Bool, join constraints, cancellation/resource-release constraints)
- `mumei-core/src/verification/support/task_ownership.rs` — Phase 1h-2 structured concurrency ownership / data-race analysis
- `mumei-core/src/mir_analysis/move_analysis.rs` — sequential MIR move analysis (Phase 1h)
- `tests/test_concurrency.rs`, `benchmarks/concurrency/task_*.mm` — regression fixtures (success cases and `expected: FAIL` counterexamples)
- `mumei-core/src/ast.rs` — `collect_from_expr` traverses generics within Task/TaskGroup
- `mumei-emit-llvm/src/codegen.rs` — Task/TaskGroup LLVM IR generation (synchronous compilation)
