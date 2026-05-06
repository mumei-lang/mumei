---
name: build
description: Build verified Mumei source into LLVM IR artifacts using mumei build --emit llvm-ir after verification succeeds.
---

Given verified `.mm` source, generate LLVM IR artifacts. Do not build unverified logic unless the user explicitly asks for a failing build diagnostic.

# Step 1: Confirm verification succeeded

Action:
    Run or reuse the **verify** skill. Confirm the report has no failed proof obligations.

Expectation:
    Every atom verifies with Z3 or accepted imported certificates.

Result:
    If verification passed, proceed to Step 2. If it failed, run **diagnose** instead of building.

# Step 2: Run LLVM IR build

Action:
    Invoke `mumei build` with the LLVM IR emit target.

Expectation:
    The compiler verifies the source again, emits `.ll` artifacts, and writes any report artifacts next to the output base.

Result:
    Build succeeds with generated LLVM IR files, or fails with a verification/codegen diagnostic to interpret.

```bash
mumei build input.mm -o katana --emit llvm-ir
```

With cargo from the compiler checkout:

```bash
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo run -- build input.mm -o katana --emit llvm-ir
```

MCP equivalent:

```text
forge_blade(source_code, output_name)
```

# Step 3: Confirm artifacts

Action:
    Inspect the generated output base and collect `.ll` files, `report.json`, or other build artifacts.

Expectation:
    LLVM IR files are present and correspond to the requested atoms/output name.

Result:
    Report artifact paths and summarize verification/build status.

# Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| file | path | yes | | `.mm` source file |
| output | path/string | no | `katana` | Output base name |
| emit | string | no | `llvm-ir` | Emit target |
| source_code | string | no | | Inline source for MCP `forge_blade` |
| allow_lean_verified | flag | no | off | Accept Lean-backed import certificates |
