# Mumei (無銘) [![GitHub](https://img.shields.io/github/stars/mumei-lang/mumei?style=social)](https://github.com/mumei-lang/mumei)

**Mathematical Proof-Driven Programming Language**

Mumei formally verifies every function with Z3 before compiling to LLVM IR and transpiling to Rust / Go / TypeScript.

> parse → resolve → monomorphize → lower_to_hir → **verify (Z3)** → codegen (LLVM IR) → transpile

```mumei
type Nat = i64 where v >= 0;

atom increment(n: Nat)
  requires: n >= 0;
  ensures: result >= 1;
  body: n + 1;

// Explicit return types for Str, f64, enums (Plan 18)
atom greet(name: Str) -> Str
  requires: true;
  ensures: true;
  body: "Hello, " + name;
```

```mumei
// Side effects are verified at compile time — undeclared effects won't compile.
effect FileWrite;
effect Log;

atom write_log(msg: Nat)
    effects: [FileWrite, Log];
    requires: msg >= 0;
    ensures: result == msg;
    body: {
        perform FileWrite.write(msg);
        perform Log.info(msg);
        msg
    };
```

```mumei
// Algebraic laws on traits — Z3 proves every impl satisfies them.
trait Comparable {
    fn leq(a: Self, b: Self) -> bool;
    law reflexive: leq(x, x) == true;
    law transitive: leq(a, b) && leq(b, c) => leq(a, c);
}

impl Comparable for i64 {
    fn leq(a: i64, b: i64) -> bool { a <= b }
}
```

---

## Install

```bash
# One-liner (macOS / Linux)
curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash

# Homebrew
brew install mumei-lang/mumei/mumei

# Specific version
curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash -s -- --version v0.2.0
```

No Rust toolchain required. Detects OS/arch automatically.

<details>
<summary>Build from source</summary>

```bash
# macOS
brew install llvm@17 z3
# Linux
sudo apt-get install -y libz3-dev llvm-17-dev libclang-17-dev

cargo build --release   # -> target/release/mumei
cargo install --path .  # -> ~/.cargo/bin/mumei

# Or auto-install Z3/LLVM
mumei setup && source ~/.mumei/env
```

</details>

---

## Getting Started

```bash
mumei init my_app
cd my_app
mumei build src/main.mm -o dist/output
```

### CLI

| Command | Description |
|---------|-------------|
| `mumei build <file> -o <out>` | Verify + codegen + transpile |
| `mumei verify <file>` | Z3 verification only |
| `mumei check <file>` | Parse + resolve (fast, no Z3) |
| `mumei init <name>` | Generate project template |
| `mumei add <dep>` | Add dependency (path / git / registry) |
| `mumei publish` | Publish to local registry |
| `mumei setup` | Download Z3 + LLVM toolchain |
| `mumei inspect` | Show development environment |
| `mumei infer-effects <file>` | Infer required effects (JSON output) |
| `mumei infer-contracts <file>` | Infer contracts for all atoms (JSON output) |
| `mumei repl` | Interactive REPL |
| `mumei doc <file> -o <dir>` | Generate HTML/Markdown documentation |
| `mumei lsp` | Start LSP server |

---

## Features

| Category | Highlights |
|----------|-----------|
| **Types** | Refinement types (`i64 where v >= 0`), Structs, Enums (ADT), Generics, explicit return types (`-> Str`) |
| **Verification** | Pre/postconditions, [loop invariants + termination proof](docs/LANGUAGE.md#termination-checking), `forall`/`exists` quantifiers, [temporal effect Z3 probes](docs/ARCHITECTURE.md#stateful-effects-temporal-effect-verification) |
| **Traits** | [Algebraic laws verified by Z3](docs/LANGUAGE.md#trait-definitions-with-laws) (`law reflexive: leq(x, x) == true`) |
| **Ownership** | [`ref` / `ref mut` / `consume`](docs/LANGUAGE.md#ownership-and-borrowing) with Z3 aliasing prevention, MIR-based move analysis |
| **Concurrency** | `async`/`await`, `task_group:all`/`task_group:any`, [deadlock-free proof via resource hierarchy](docs/LANGUAGE.md#asyncawait-and-resource-hierarchy) |
| **Effects** | Compile-time side-effect verification, `perform`/`effects:`, effect hierarchy, parameterized effects, [effect polymorphism (`<E: Effect>`)](docs/LANGUAGE.md), [capability security](docs/CAPABILITY_SECURITY.md), stateful effects with temporal ordering |
| **Lambda** | First-class closures `\|x, y\| x + y`, capture analysis, transpiles to Rust / TS / Go |
| **Safety** | `trusted` / `unverified` atoms, taint analysis, BMC + inductive invariant, [`call_with_contract`](docs/LANGUAGE.md#higher-order-functions-phase-a) for higher-order function verification |
| **FFI** | `extern "Rust"` / `extern "C"` blocks, handle-based memory management (`json_free`, `http_free`), Str type interop |
| **Std Library** | Option, Result, List, BoundedArray, Vector, HashMap, JSON, HTTP, sort algorithms, effect definitions |
| **Output** | LLVM IR + Rust + Go + TypeScript transpiler |
| **Tooling** | LSP server, VS Code extension, `mumei.toml` manifest, dependency manager, MCP server, Streamlit Visualizer, semantic feedback (bilingual EN/JP) |

<details>
<summary><b>More examples</b></summary>

**Loop invariant + termination proof** — Z3 proves the loop terminates and the invariant holds inductively:

```mumei
atom sum_up_to(n: i64)
    requires: n >= 0;
    ensures: result >= 0;
    body: {
        let s = 0;
        let i = 0;
        while i < n
        invariant: s >= 0 && i <= n
        decreases: n - i
        {
            s = s + i;
            i = i + 1;
        };
        s
    };
```

**Higher-order function contracts** — `contract(f)` lets Z3 verify generic callbacks without `trusted`:

```mumei
atom apply_twice(x: i64, f: atom_ref(i64) -> i64)
    requires: x >= 0;
    ensures: result >= 0;
    contract(f): requires: x >= 0, ensures: result >= 0;
    body: {
        let first = call(f, x);
        call(f, first)
    };
```

**Deadlock-free concurrency** — resource priorities are verified at compile time:

```mumei
resource db   priority: 1 mode: exclusive;
resource cache priority: 2 mode: shared;

async atom transfer(amount: i64)
    resources: [db, cache];
    requires: amount >= 0;
    ensures: result >= 0;
    body: {
        acquire db { acquire cache { amount } }
    };
```

See [Language Reference](docs/LANGUAGE.md) for full syntax documentation.

</details>

### Rich Diagnostics

Multi-span diagnostics powered by [miette](https://crates.io/crates/miette) — multiple related source locations, compound constraint decomposition, and expression-level dataflow tracking on every error.

**Multi-span output** — primary error + related constraint/dataflow locations:

```
  × Verification Error: Effect constraint not satisfied for 'perform SafeFileRead.read(path)'
   ╭─[examples/server.mm:15:9]
14 │         let path = "/tmp/" + user_id + "/config.txt";
15 │         perform SafeFileRead.read(path);
   ·         ─────────────────────────────── constraint violated here
16 │
   ╰────
   ╭─[examples/server.mm:14:20]
14 │         let path = "/tmp/" + user_id + "/config.txt";
   ·                    ──────────────────────────────────── path constructed here
   ╰────
   ╭─[std/file.mm:3:5]
 3 │     where starts_with(path, "/tmp/") && not_contains(path, "..");
   ·           ──────────────────────────────────────────────────────── constraint defined here
   ╰────
  help: Sub-constraint [2/2] 'not_contains(path, "..")' may be violated.
        user_id に ".." が含まれていないか確認してください。
```

**Compound constraint decomposition** — each `&&`-joined sub-constraint is individually evaluated:

```
  × Verification Error: Postcondition (ensures) is not satisfied.
   ╭─[examples/basic.mm:5:1]
 4 │   ensures: result > 0;
 5 │   body: x - 1;
   ·   ──────────── verification failed here
 6 │
   ╰────
  help: ensures の条件を確認してください。body の返り値が事後条件を満たすか検討してください
```

**MCP JSON output** — structured data for AI agent consumption:

```json
{
  "failure_type": "precondition_violated",
  "semantic_feedback": {
    "violated_constraints": [{
      "param": "path",
      "constraint": "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")",
      "sub_constraints": [
        {"index": 0, "raw": "starts_with(path, \"/tmp/\")", "satisfied": true},
        {"index": 1, "raw": "not_contains(path, \"..\")", "satisfied": false,
         "explanation": "'path' must not contain \"..\""}
      ]
    }],
    "data_flow": [
      {"step": "concat", "line": 14, "col": 20},
      {"step": "perform", "line": 15, "col": 9,
       "constraint": "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")"}
    ],
    "related_locations": [
      {"file": "examples/server.mm", "line": 14, "label": "path constructed here"},
      {"file": "std/file.mm", "line": 3, "label": "constraint defined here"}
    ]
  }
}
```

LSP diagnostics include `relatedInformation` for multi-location errors, enabling IDE inline display of all related spans.

---

## Self-Healing Loop (AI + Z3)

Mumei's Self-Healing loop combines AI (LLM) and Z3 formal verification to automatically fix code. Rich Diagnostics provide the AI with structured semantic feedback — including sub-constraint decomposition, dataflow traces, and multi-span source locations — enabling precise, targeted repairs.

### E2E Flow

```
AI generates .mm code
        |
        v
validate_logic (Z3 verification + Rich Diagnostics)
        |
   [fail?] -----> AI receives semantic feedback JSON
        |              - violated_constraints with sub_constraints (satisfied/violated)
        |              - data_flow trace (expression-level source locations)
        |              - related_locations (constraint definition sites)
        |         AI analyzes structured feedback -> generates targeted fix -> re-verify (loop)
        |
   [pass!]
        v
execute_mm (full build: LLVM IR + Rust/Go/TypeScript)
```

### Interactive Flow (Layered Architecture)

The E2E flow and Visualizer serve complementary but independent roles:

| | E2E Flow (MCP) | Visualizer (Streamlit) |
|---|---|---|
| **Purpose** | Data channel for AI to autonomously run verify-fix loops | Observation tool for humans to visually inspect verification state |
| **Consumer** | AI (Claude Desktop, etc.) | Human (developer) |
| **Data Source** | Rich Diagnostics JSON + stderr in MCP responses | Reads report.json file |
| **Real-time** | Immediate on every tool call | Streamlit page reload / rerun |
| **Rich Diagnostics** | `sub_constraints`, `data_flow`, `related_locations` in JSON | Constraint mappings + counterexample display |

**Recommended Architecture:**

```
AI (Claude Desktop etc.)
  | MCP
  v
validate_logic / execute_mm / forge_blade
  |
  v
mumei compiler (Z3 verification + Rich Diagnostics)
  |
  +---> [Always] Include verification results + semantic feedback in MCP response
  |              - violated_constraints with sub_constraint decomposition
  |              - data_flow trace for expression-level tracking
  |              - related_locations for multi-span context
  |              -> AI can run autonomous fix loops with this alone
  |
  +---> [Optional] Copy to visualizer/report.json
              -> Streamlit dashboard for human state inspection
```

### Demo 1: Self-Healing of safe_divide (Division by Zero)

**Step a.** AI generates initial code (insufficient precondition):

```mumei
type Nat = i64 where v >= 0;

atom safe_divide(a: Nat, b: Nat)
  requires: a >= 0;
  ensures: result >= 0;
  body: { a / b };
```

**Step b.** `validate_logic` runs Z3 verification -> fails with Rich Diagnostics:

```
$ mumei verify input.mm

  × Verification Error: Potential division by zero.
   ╭─[input.mm:6:11]
 5 │   ensures: result >= 0;
 6 │   body: { a / b };
   ·           ─────── division by zero possible here
 7 │
   ╰────
  help: Add a condition divisor != 0 to requires

  Verification: 0 passed, 1 failed
```

**Step c-d.** AI receives semantic feedback JSON and generates fix:

```json
{
  "failure_type": "division_by_zero",
  "semantic_feedback": {
    "counter_example": {"dividend": "0", "divisor": "0"},
    "suggestion": "Add a condition divisor != 0 to requires"
  }
}
```

```mumei
atom safe_divide(a: Nat, b: Nat)
  requires: a >= 0 && b > 0;   // <- fix: added b > 0
  ensures: result >= 0;
  body: { a / b };
```

**Step e.** Re-verify -> passes:

```
$ mumei verify input.mm
  'safe_divide': verified

  Verification passed: 1 item(s) verified
```

**Step f.** Full build generates Rust / Go / TypeScript:

```
$ mumei build input.mm -o katana

  Blade forged successfully with 1 atoms.
  Done. Created: katana.rs, katana.go, katana.ts
```

### Demo 2: Self-Healing with Compound Constraint Decomposition (Path Safety)

**Step a.** AI generates code with compound constraint violation:

```mumei
type SafePath = Str where starts_with(v, "/tmp/") && not_contains(v, "..");

effect SafeFileRead;

atom read_config(user_id: Str)
    effects: [SafeFileRead];
    requires: true;
    ensures: true;
    body: {
        let path = "/tmp/" + user_id + "/config.txt";
        perform SafeFileRead.read(path);
    };
```

**Step b.** `validate_logic` returns Rich Diagnostics with multi-span + sub-constraint decomposition:

```
  × Verification Error: Effect constraint not satisfied for 'perform SafeFileRead.read(path)'
   ╭─[input.mm:11:9]
10 │         let path = "/tmp/" + user_id + "/config.txt";
11 │         perform SafeFileRead.read(path);
   ·         ─────────────────────────────── constraint violated here
   ╰────
   ╭─[input.mm:10:20]
10 │         let path = "/tmp/" + user_id + "/config.txt";
   ·                    ──────────────────────────────────── path constructed here
   ╰────
  help: Sub-constraint [2/2] 'not_contains(path, "..")' may be violated.
        user_id に ".." が含まれていないか確認してください。
```

**Step c.** AI receives structured semantic feedback JSON:

```json
{
  "failure_type": "precondition_violated",
  "semantic_feedback": {
    "violated_constraints": [{
      "param": "path",
      "constraint": "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")",
      "sub_constraints": [
        {"index": 0, "raw": "starts_with(path, \"/tmp/\")", "satisfied": true},
        {"index": 1, "raw": "not_contains(path, \"..\")", "satisfied": false,
         "explanation": "'path' must not contain \"..\""}
      ]
    }],
    "data_flow": [
      {"step": "concat", "line": 10, "col": 20},
      {"step": "perform", "line": 11, "col": 9,
       "constraint": "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")"}
    ],
    "related_locations": [
      {"file": "input.mm", "line": 10, "label": "path constructed here"}
    ]
  }
}
```

AI identifies: sub-constraint `[1/2]` is satisfied (path starts with "/tmp/"), but `[2/2]` is violated (user_id could contain ".."). Fix: add requires clause to validate user_id.

**Step d.** AI generates fix:

```mumei
atom read_config(user_id: Str)
    effects: [SafeFileRead];
    requires: not_contains(user_id, "..");   // <- fix: validate user_id
    ensures: true;
    body: {
        let path = "/tmp/" + user_id + "/config.txt";
        perform SafeFileRead.read(path);
    };
```

**Step e.** Re-verify -> passes.

### Setup

```bash
# 1. Start Ollama container
docker compose up -d
docker exec mumei-ollama ollama pull qwen3.5

# 2. Configure environment variables
cp .env.example .env
# Uncomment Pattern 1 (Ollama) in .env

# 3. Install Python dependencies
pip install -r requirements.txt

# 4. Run Self-Healing loop
python self_healing.py

# 5. Start MCP server (for use with Claude Desktop, etc.)
python mcp_server.py
```

### MCP Tools

| Tool | Description |
|------|-------------|
| `forge_blade` | Verify + code generation in one step |
| `self_heal_loop` | Run autonomous fix loop |
| `validate_logic` | Z3 verification only (returns counter-example data) |
| `execute_mm` | General-purpose build / check execution |
| `get_inferred_effects` | Pre-check: infer required effects before writing code |
| `get_allowed_effects` | Query current effect boundary for the session |
| `set_allowed_effects` | Override effect boundary dynamically |
| `self_heal_with_effects` | Effect-aware self-healing loop with boundary enforcement |

### Visualizer Dashboard (Optional)

A Streamlit-based Visualizer for monitoring verification results and Self-Healing history in real-time.

| Scenario | E2E Flow | Visualizer | Config |
|---|---|---|---|
| AI-only autonomous fix | Yes | No | `ENABLE_VISUALIZER_SYNC=false` |
| Human monitors dashboard while AI works | Yes | Yes | `ENABLE_VISUALIZER_SYNC=true` |
| Manual compiler run + inspect results | No | Yes | Run `mumei build` directly |

**Setup:**

```bash
# 1. Enable Visualizer sync in .env
echo "ENABLE_VISUALIZER_SYNC=true" >> .env

# 2. Start Streamlit
pip install streamlit
streamlit run visualizer/app.py

# 3. Run MCP tools or self_healing.py
#    -> report.json is automatically copied to visualizer/ and reflected in the dashboard
```

**Features:**

- **Latest Report View**: Structured display of Z3 verification results + counterexample field (variable values)
- **Self-Healing History**: Time-series display of each iteration result (with pass/fail summary)
- **AI Fix Suggestion**: Auto-generated fix hints on verification failure

**Demo Recording:**

[MCP + Rich Diagnostics Demo](https://github.com/user-attachments/assets/0f0594a4-8946-422c-9d54-bd81af45fc14)

[Demo 2: Compound Constraint Decomposition (Path Safety)](https://github.com/user-attachments/assets/cc5f7d93-a759-418d-9b46-520500c38672)

[Visualizer Demo](https://github.com/user-attachments/assets/b36271b7-eeb0-452c-97ca-4045babd23a4)

> The Visualizer is a monitoring tool for the E2E flow. Since AI completes its work via MCP responses alone (including Rich Diagnostics JSON with `sub_constraints`, `data_flow`, and `related_locations`), it does not depend on the Visualizer. Use it only when a human wants to observe.

---

## Documentation

| Document | Content |
|----------|---------|
| [Language Reference](docs/LANGUAGE.md) | Types, generics, traits, ownership, async |
| [Standard Library](docs/STDLIB.md) | Option, Result, List, BoundedArray, sort |
| [Examples & Tests](docs/EXAMPLES.md) | Verification suite, pattern matching, negative tests |
| [Architecture](docs/ARCHITECTURE.md) | Compiler internals |
| [Toolchain](docs/TOOLCHAIN.md) | CLI commands, package management |
| [Roadmap](docs/ROADMAP.md) | Strategic roadmap |
| [Capability Security](docs/CAPABILITY_SECURITY.md) | Effect-based capability security evaluation |
| [Changelog](docs/CHANGELOG.md) | Release history |

---

## Development

```bash
pip install pre-commit && pre-commit install
cargo test
cargo clippy
```

---

## License

[Apache-2.0 license](LICENSE)
