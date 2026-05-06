---
name: certify
description: Generate and verify Mumei proof certificates with mumei verify --proof-cert and mumei verify-cert.
---

Given `.mm` source whose verification succeeds, produce a proof certificate and re-check it against the current source.

# Step 1: Generate a proof certificate

Action:
    Run `mumei verify --proof-cert`, optionally with an explicit output path.

Expectation:
    Verification succeeds and writes a `.proof.json` certificate containing atom proof metadata.

Result:
    If certificate generation succeeds, proceed to Step 2. If verification fails, run **diagnose**.

```bash
mumei verify input.mm --proof-cert --output input.proof.json
```

With cargo from the compiler checkout:

```bash
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo run -- verify input.mm --proof-cert --output input.proof.json
```

# Step 2: Inspect certificate structure

Action:
    Parse the generated `.proof.json` and confirm it includes module metadata, atom entries, content/proof hashes, dependencies/effects, and Z3 result fields.

Expectation:
    The certificate is valid JSON and every atom entry reflects the just-verified source.

Result:
    If the structure is valid, proceed to Step 3.

```bash
python -m json.tool input.proof.json >/dev/null
```

# Step 3: Re-verify the certificate

Action:
    Run `mumei verify-cert` against the certificate and source file.

Expectation:
    The command reports that the certificate matches current source hashes and proof status.

Result:
    Report certificate validity. If atoms changed, regenerate the certificate.

```bash
mumei verify-cert input.proof.json input.mm
```

When accepting Lean-backed imported certificates:

```bash
mumei verify-cert input.proof.json input.mm --allow-lean-verified
```

# Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| source_file | path | yes | | `.mm` source file |
| cert_file | path | no | `<source>.proof.json` | Output/input certificate path |
| allow_lean_verified | flag | no | off | Accept `z3_check_result = "lean_verified"` |
