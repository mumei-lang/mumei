---
name: verify
description: Run Z3 verification for Mumei .mm source using mumei verify --json or the validate_logic MCP tool, then interpret proof status and counterexamples.
---

Given Mumei `.mm` source, prove every atom contract with Z3 and return a structured pass/fail result. Prefer the MCP `validate_logic` tool when available; otherwise use the CLI.

# Step 1: Prepare the `.mm` source

Action:
    Locate the target `.mm` file or write the provided source to a temporary `.mm` file.
    Confirm it contains atoms with `requires`, `ensures`, optional `effects`, and valid module imports.

Expectation:
    Source is syntactically ready for `mumei verify`.

Result:
    If the source is ready, proceed to Step 2. If dependencies or imports are missing, report them before verification.

# Step 2: Run verification

Action:
    Run Z3 verification with machine-readable output.

Expectation:
    The CLI prints a JSON report, or the MCP tool returns a verification section with JSON and semantic feedback.

Result:
    Verification succeeds when every proof obligation is discharged. Continue to Step 3 to interpret.

```bash
mumei verify input.mm --json
```

With cargo from the compiler checkout:

```bash
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo run -- verify input.mm --json
```

MCP equivalent:

```text
validate_logic(source_code)
```

# Step 3: Interpret the result

Action:
    Parse `semantic_feedback`, `machine_readable`, `failure_type`, `actions`, and `counter_example`.

Expectation:
    On success, report all constraints satisfied. On failure, identify the atom, violated obligation, and concrete counterexample values when present.

Result:
    If verification passed, downstream skills may run: **build** or **certify**.
    If verification failed, run **diagnose**.

# Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| source_code | string | no | | Inline `.mm` source for MCP verification |
| file | path | no | | Existing `.mm` source file |
| json | flag | no | on | Request machine-readable CLI output |
| strict_imports | flag | no | off | Require valid import certificates |
| allow_lean_verified | flag | no | off | Accept `lean_verified` imported certificates |

Either `source_code` or `file` must be provided.
