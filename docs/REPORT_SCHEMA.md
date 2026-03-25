# report.json Schema

This document defines the JSON schema for `report.json` output by `mumei verify`.
External tools (e.g., [mumei-agent](https://github.com/mumei-lang/mumei-agent)) depend on this schema.

## Output Methods

- `mumei verify --json file.mm` — outputs report to stdout
- `mumei verify --report-dir <dir> file.mm` — writes report.json to specified directory

## Top-level Fields

| Field | Type | Required | Description |
|---|---|---|---|
| `status` | `"success" \| "failed"` | Yes | Verification result |
| `atom` | `string` | Yes | Atom name being verified |
| `input_a` | `string` | No | First input description (legacy; prefer `counterexample`) |
| `input_b` | `string` | No | Second input description (legacy; prefer `counterexample`) |
| `reason` | `string` | Yes | Human-readable result description |
| `violation_type` | `string` | No | `"effect_mismatch"`, `"effect_propagation"`, etc. |
| `effect_violation` | `object` | No | Effect violation details (see below) |
| `semantic_feedback` | `object` | No | Rich diagnostics (see below) |
| `counterexample` | `object` | No | Z3 counter-example values |
| `failure_type` | `string` | No | Failure category code |
| `suggestion` | `string` | No | Fix suggestion text (counterexample や unsat core を踏まえた動的な修正提案が含まれる場合がある) |
| `span` | `object` | No | Source location (`file`, `line`, `col`, `len`) |
| `type_definition_locations` | `array` | No | Constraint source locations |

## semantic_feedback.structured_unsat_core

When `failure_type` is `"invariant_violated"`, the `semantic_feedback` object includes a `structured_unsat_core` array. Each element is a `StructuredLabel` object:

| Field | Type | Description |
|---|---|---|
| `constraint_type` | `string` | One of: `"requires"`, `"refined_type"`, `"struct_field"`, `"quantifier"`, `"u64_nonneg"` |
| `param` | `string \| null` | Parameter name (for `refined_type`, `struct_field`, `u64_nonneg`) |
| `type_name` | `string \| null` | Type name (for `refined_type`) |
| `field` | `string \| null` | Field name (for `struct_field`) |
| `description` | `string` | Human-readable bilingual description |

The existing `conflicting_constraints` (array of description strings) and `raw_unsat_core` (array of raw Z3 label strings) fields are preserved for backward compatibility.

## semantic_feedback Object

| Field | Type | Description |
|---|---|---|
| `violated_constraints` | `array` | Array of constraint violation objects |
| `violated_constraints[].param` | `string` | Parameter name |
| `violated_constraints[].type` | `string` | Type name |
| `violated_constraints[].value` | `string` | Violating value |
| `violated_constraints[].constraint` | `string` | Constraint expression |
| `violated_constraints[].explanation` | `string` | Human-readable explanation |
| `violated_constraints[].suggestion` | `string` | Fix suggestion |
| `violated_constraints[].sub_constraints` | `array` | Decomposed sub-constraints |
| `data_flow` | `array` | Expression-level dataflow trace |
| `related_locations` | `array` | Multi-span source locations |

## effect_violation Object

| Field | Type | Description |
|---|---|---|
| `declared_effects` | `array` | Effects declared by the atom |
| `required_effect` | `string` | Effect required by the operation |
| `source_operation` | `string` | The operation that triggered the violation |
| `resolution_paths` | `array` | Suggested resolution options |
| `caller` | `string` | Caller atom name (for propagation) |
| `callee` | `string` | Callee atom name (for propagation) |
| `caller_effects` | `array` | Caller's declared effects |
| `callee_effects` | `array` | Callee's required effects |
| `missing_effects` | `array` | Effects missing from caller |
