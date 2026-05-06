---
name: diagnose
description: Diagnose Mumei verification failures by parsing semantic_feedback, machine_readable failure_type/actions, and counter_example data from verify JSON output.
---

Given a failed `mumei verify --json` report, explain why verification failed and propose minimal repairs to the `.mm` source or contracts.

# Step 1: Parse verification JSON

Action:
    Load the report produced by `mumei verify --json`, `report.json`, or `validate_logic`.
    Preserve the raw `semantic_feedback`, `machine_readable`, and `counter_example` sections.

Expectation:
    The report identifies failed atoms, violated constraints, and structured diagnostics. The `machine_readable` payload follows the schema built by verification's machine-readable feedback helpers, including fields such as `failure_type`, `actions`, related locations, data flow, and conflicting constraints.

Result:
    If structured fields are present, proceed to Step 2. If not, fall back to stderr and compiler diagnostics.

# Step 2: Interpret failure details

Action:
    Classify the failure using `failure_type`, then map counterexample values to the failing `requires`, `ensures`, match arm, effect declaration, or ownership/effect rule.

Expectation:
    Common failure types include:
    `postcondition_violated`, `precondition_violated`, `division_by_zero`, `trait_law_violated`, `linearity_violated`, `invariant_violated`, `exhaustiveness_failed`, `resource_conflict`, and `effect_not_allowed`.

Result:
    Produce a concise diagnosis with the atom name, violated condition, counterexample, and likely root cause.

```bash
mumei verify input.mm --json > report.json
python -m json.tool report.json
```

# Step 3: Generate repair suggestions

Action:
    Propose minimal edits. Prefer fixing the body when the contract is correct; only relax contracts when the stated guarantee is too strong.

Expectation:
    Suggestions are concrete and tied to report evidence:
    add guards for division by zero, cover missing enum cases, add declared effects, strengthen invariants, or update the body to satisfy `ensures`.

Result:
    Return an actionable repair plan. After edits, rerun **verify**.

# Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| report | JSON object/string | no | | Parsed or raw verification report |
| report_file | path | no | `report.json` | Machine-readable report file |
| source_file | path | no | | Original `.mm` source for line/context lookup |
| counter_example | JSON object | no | | Explicit counterexample payload |
