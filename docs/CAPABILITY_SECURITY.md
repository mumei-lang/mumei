# Capability Security Evaluation

> Evaluation of Mumei's current parameterized effect system for capability-based security.

## 1. Current Approach Assessment

### Strengths

- **Compile-time verification via Z3**: All effect constraints are verified before code generation, ensuring zero runtime overhead for security enforcement.
- **Parameterized effects with `where` constraints**: Effects like `FileRead(path: Str) where starts_with(path, "/tmp/")` allow fine-grained access control at the type level.
- **Effect containment proof**: Z3 proves `UsedEffects(body) ⊆ AllowedEffects(signature)` for every atom, preventing undeclared side effects.
- **Effect hierarchy (subtyping)**: Composite effects (`IO includes: [FileRead, FileWrite, Console]`) enable coarse-grained permission grouping.
- **Effect propagation checking**: Call chains are verified — if atom A calls atom B, then `B.effects ⊆ A.effects` must hold.

### Weaknesses

- **Limited to string constraints**: Current `check_constant_constraint()` supports `starts_with`, `contains`, `ends_with`, `not_contains`, and `matches()` (regex). No glob or arithmetic path operations.
- **No dynamic capability delegation**: An atom cannot pass a subset of its capabilities to a callee at the call site. Capabilities are statically declared per atom.
- **No capability revocation**: Once an effect is declared, it cannot be narrowed or revoked within a scope.
- **Z3 String Sort integrated**: Path constraints are verified via constant folding (Rust-side) for constants and Z3 String Sort for symbolic/dynamic paths. Dynamic string concatenation (e.g., `"/tmp/" + var + "/file.txt"`) is verified against constraints like `starts_with(path, "/tmp/")` by Z3.
- **No first-class capability objects**: Effects are names, not values. They cannot be stored in variables, passed as arguments, or pattern-matched.

### Supported Constraint Functions

From `check_constant_constraint()` in `mumei-core/src/verification.rs`:

| Function | Semantics | Example |
|---|---|---|
| `starts_with(path, prefix)` | `path` begins with `prefix` | `starts_with(path, "/tmp/")` |
| `contains(path, substr)` | `path` contains `substr` | `contains(url, "api.example.com")` |
| `ends_with(path, suffix)` | `path` ends with `suffix` | `ends_with(file, ".txt")` |
| `not_contains(path, substr)` | `path` does NOT contain `substr` | `not_contains(path, "..")` |
| `matches(path, regex)` | `path` matches `regex` pattern | `matches(path, "^/tmp/.*")` |

## 2. Evaluation Criteria

### 2.1 File Path Policies

**Scenario**: Restrict file operations to `/tmp/` directory.

```mumei
effect FileRead(path: Str) where starts_with(path, "/tmp/");

atom read_tmp(filename: Str)
    effects: [FileRead];
    requires: starts_with(filename, "/tmp/");
    ensures: result >= 0;
    body: { perform FileRead.read(filename); 0 };
```

**Result**: Supported. Z3 verifies that `filename` satisfies the `starts_with` constraint at compile time.

### 2.2 URL Whitelisting

**Scenario**: Restrict HTTP requests to HTTPS only.

```mumei
effect HttpGet(url: Str) where starts_with(url, "https://");

atom fetch_secure(url: Str)
    effects: [HttpGet];
    requires: starts_with(url, "https://");
    ensures: result >= 0;
    body: { perform HttpGet.get(url); 0 };
```

**Result**: Supported via `starts_with` constraint. More complex URL validation (domain whitelisting, path restrictions) would require `contains` or future regex support.

### 2.3 Capability Delegation

**Scenario**: Atom A has `FileRead("/")` and passes `FileRead("/tmp/")` to atom B.

```mumei
atom reader(filename: Str)
    effects: [FileRead];
    requires: starts_with(filename, "/tmp/");
    ensures: result >= 0;
    body: { perform FileRead.read(filename); 0 };

atom delegator(filename: Str)
    effects: [FileRead];
    requires: starts_with(filename, "/tmp/config/");
    ensures: result >= 0;
    body: reader(filename);
```

**Result**: Partially supported. The callee `reader` requires `starts_with(filename, "/tmp/")`, and the caller provides `starts_with(filename, "/tmp/config/")` which is a strict subset. Z3 can prove that `/tmp/config/...` implies `/tmp/...` via the `starts_with` prefix relationship. However, there is no explicit syntax for "pass a narrowed capability" — it is implicit through the `requires` contract.

### 2.4 Capability Narrowing

**Scenario**: Atom A has `FileRead("/")`, atom B gets `FileRead("/tmp/")` only.

**Result**: Supported implicitly. Each atom declares its own effect constraints via `where` clauses. The verifier checks that each atom's `perform` operations satisfy its own constraints. There is no inheritance of constraints from caller to callee — each atom is self-contained.

### 2.5 Dynamic Path Construction

**Scenario**: `"/tmp/" + user_id + "/config.txt"` verification.

```mumei
atom read_user_config(user_id: Str)
    effects: [FileRead];
    requires: starts_with(user_id, "user_");
    ensures: result >= 0;
    body: {
        let path = "/tmp/" + user_id + "/config.txt";
        perform FileRead.read(path);
        0
    };
```

**Result**: Supported (Plan 10). Z3 String Sort verifies that dynamically constructed paths satisfy effect constraints. The Perform handler uses `arg_z3_values[i]` directly when the argument has Z3 String Sort, enabling concat expressions to be checked against `starts_with`/`ends_with`/`contains` constraints.

### 2.6 Regex Policy Support

**Scenario**: Restrict file paths using regex patterns.

```mumei
effect FileRead(path: Str) where matches(path, "^/tmp/[a-z]+/.*");
```

**Result**: Supported (Plan 10). The `matches()` constraint function uses Rust's `regex` crate for constant path verification and approximates common regex patterns (anchored prefix/suffix/contains) via Z3 String Sort for symbolic verification.

### 2.7 URL Validation

**Scenario**: Enforce HTTPS-only URLs for HTTP effects.

```mumei
effect SecureHttpGet(url: Str) where starts_with(url, "https://");
effect SecureHttpPost(url: Str) where starts_with(url, "https://");

atom fetch_api()
    effects: [SecureHttpGet(url)]
    requires: true;
    ensures: result >= 0;
    body: {
        perform SecureHttpGet.get("https://api.example.com/users");
        1
    };

atom fetch_variable(api_url: Str)
    effects: [SecureHttpGet(url)]
    requires: starts_with(api_url, "https://");
    ensures: result >= 0;
    body: {
        perform SecureHttpGet.get(api_url);
        1
    };
```

**Result**: Supported (Plan 23). `SecureHttpGet` and `SecureHttpPost` are defined in `std/http.mm` with `starts_with(url, "https://")` constraints. For literal URLs, the Rust-side `check_constant_constraint()` verifies the prefix. For variable URLs, Z3 String Sort verifies that the `requires` clause implies the effect constraint. Existing `HttpGet`/`HttpPost` (without constraints) remain for backward compatibility.

## 3. Object-Based Capability Model (Alternative)

If the current approach proves insufficient, an object-based capability model could be introduced:

```mumei
// Hypothetical syntax for first-class capabilities
type FileCap = capability FileRead(path: Str) where starts_with(path, "/tmp/");

atom read_config(cap: FileCap, filename: Str)
    requires: starts_with(filename, "/tmp/");
    body: { perform cap.read(filename); 0 };

atom main()
    effects: [FileRead];
    body: {
        let cap = grant FileRead where starts_with(path, "/tmp/");
        read_config(cap, "/tmp/config.txt")
    };
```

**Advantages**:
- Capabilities are first-class values that can be stored, passed, and pattern-matched
- Explicit delegation: `grant` creates a capability with specific constraints
- Narrowing: A capability can be narrowed before passing to a callee
- Revocation: Capabilities can be dropped or scoped to a block

**Disadvantages**:
- Significant language and compiler complexity increase
- Requires new AST nodes, type system extensions, and Z3 encoding
- Runtime representation needed for capability objects
- Breaking change to the effect system

## 4. Recommendation

**Option A: Continue with parameterized effects + Z3 (Recommended)**

The current approach is sufficient for the primary use cases:
- File path restriction via `starts_with`/`contains`/`ends_with`
- URL whitelisting via `starts_with`
- Effect propagation checking through call chains
- Compile-time verification with zero runtime overhead

**Rationale**:
1. The four supported constraint functions cover the majority of real-world security policies (file sandboxing, URL filtering, environment variable access control).
2. Dynamic path construction (the main gap) will be addressed by Z3 String Sort migration, which is already planned in the roadmap.
3. An object-based capability model would require a major language redesign with unclear benefit for the current target audience (AI-generated API scripts).
4. The implicit capability narrowing through `requires` contracts provides adequate delegation semantics.

**Next Steps**:
1. ~~Complete Z3 String Sort integration~~ (Done: Plan 5 + Plan 10)
2. ~~Add `matches()` regex constraint function~~ (Done: Plan 10)
3. Monitor user feedback for capability delegation needs
4. Explore full Z3 regex API (`str.in_re`) when z3 crate exposes it

## 5. Test Results

See `tests/test_capability_evaluation.mm` for evaluation test cases covering:
- Simple path constraint (Test 1)
- Path delegation through call chain (Test 2)
- Pure computation without effects (Test 3)
- Multiple effects with containment (Test 4)
