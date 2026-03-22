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
| LLVM codegen | ✅ Implemented (body compiled synchronously) |
| Parser tests | ✅ Implemented (6 tests: task, task_group, :all, :any, unknown panic) |
| Unique ID (Task) | ✅ Implemented (TASK_COUNTER prevents env key collision) |
| Runtime scheduler | ❌ Not implemented |
| Task cancellation | ❌ Not implemented |
| Channel types | ❌ Not implemented |

## Safety Guarantees

| Property | Verification Method | Status |
|---|---|---|
| Deadlock prevention | Z3 verification of resource hierarchy (priority) | ✅ Implemented |
| Resource hold across await | Detect await inside acquire block | ✅ Implemented |
| Async recursion depth | BMC unroll limit check | ✅ Implemented |
| Parent task termination constraint | Z3 verification of TaskGroup join semantics | ✅ Implemented |
| Task cancellation safety | Remaining task cleanup on Any completion | ❌ Future |

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
3. **Task cancellation**: Safe cancellation of remaining tasks on `Any` completion
4. **Timeouts**: Timeout specification for task groups
5. **LLVM codegen**: LLVM coroutine transformation for task scheduling code
6. **TaskGroup unique ID**: Prevent Z3 variable name collision across multiple TaskGroups (TASK_GROUP_COUNTER)
7. **Return type inference**: Auto-infer return type from Task body
8. **Result binding syntax**: Syntax to bind `task_group` results to variables

## Related Files

- `src/parser.rs` — `Task`, `TaskGroup`, `JoinSemantics` definitions + parsing + tests
- `src/verification.rs` — Z3 structured concurrency verification (symbolic Bool, join constraints)
- `src/ast.rs` — `collect_from_expr` traverses generics within Task/TaskGroup
- `src/codegen.rs` — Task/TaskGroup LLVM IR generation (synchronous compilation)
