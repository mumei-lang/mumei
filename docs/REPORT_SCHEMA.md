# report.json Schema

This document defines the JSON schema for `report.json` output by `mumei verify`.
External tools (e.g., [mumei-agent](https://github.com/mumei-lang/mumei-agent)) depend on this schema.

## Output Methods

- `mumei verify --json file.mm` — outputs report to stdout
- `mumei verify --report-dir <dir> file.mm` — writes report.json to specified directory

## Rich Diagnostics

Human-facing diagnostics are powered by [miette](https://crates.io/crates/miette): multi-span source locations, compound constraint decomposition, and expression-level dataflow tracking can all be attached to one verification error.

### Multi-span output

```text
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

### Compound constraint decomposition

Each `&&`-joined sub-constraint can be evaluated and reported separately:

```text
  × Verification Error: Postcondition (ensures) is not satisfied.
   ╭─[examples/basic.mm:5:1]
 4 │   ensures: result > 0;
 5 │   body: x - 1;
   ·   ──────────── verification failed here
 6 │
   ╰────
  help: ensures の条件を確認してください。body の返り値が事後条件を満たすか検討してください
```

### Rich MCP JSON example

MCP clients consume the same information through `semantic_feedback`:

```json
{
  "failure_type": "precondition_violated",
  "semantic_feedback": {
    "violated_constraints": [
      {
        "param": "path",
        "constraint": "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")",
        "sub_constraints": [
          {
            "index": 0,
            "raw": "starts_with(path, \"/tmp/\")",
            "satisfied": true
          },
          {
            "index": 1,
            "raw": "not_contains(path, \"..\")",
            "satisfied": false,
            "explanation": "'path' must not contain \"..\""
          }
        ]
      }
    ],
    "data_flow": [
      {
        "step": "concat",
        "line": 14,
        "col": 20
      },
      {
        "step": "perform",
        "line": 15,
        "col": 9,
        "constraint": "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")"
      }
    ],
    "related_locations": [
      {
        "file": "examples/server.mm",
        "line": 14,
        "label": "path constructed here"
      },
      {
        "file": "std/file.mm",
        "line": 3,
        "label": "constraint defined here"
      }
    ]
  }
}
```

LSP diagnostics include `relatedInformation` for multi-location errors, enabling IDE inline display of all related spans.

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

## semantic_feedback.minimal_unsat_core

When `failure_type` is `"invariant_violated"` and Z3 reports a contradiction, the `semantic_feedback` object includes a `minimal_unsat_core` array containing a deletion-minimal subset of tracked constraints that is still contradictory.
This diagnostic helps identify which constraints should be relaxed when revising a specification.

| Field | Type | Description |
|---|---|---|
| `minimal_unsat_core` | `array[string]` | Minimal set of constraint labels causing contradiction |
| `minimal_core_size` | `number` | Size of minimal core |
| `total_core_size` | `number` | Size of full unsat core |
| `reduction_ratio` | `number` | Ratio of minimal to full core (0.0 to 1.0) |
| `suggestion` | `string` | Human-readable suggestion for fixing the contradiction |

Example:

```json
{
  "failure_type": "invariant_violated",
  "minimal_unsat_core": ["track_refined_type_n::Pos", "track_requires"],
  "minimal_core_size": 2,
  "total_core_size": 5,
  "reduction_ratio": 0.4,
  "suggestion": "Minimal conflicting constraints: [track_refined_type_n::Pos, track_requires]. Consider relaxing one of these."
}
```

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

## cross_spec.json Schema

When cross-specification verification is enabled with `mumei verify --cross-spec-verify`, Mumei writes `cross_spec.json` in the report directory.

| Field | Type | Description |
|---|---|---|
| `contract_consistency` | `array` | Contract consistency check results between caller/callee atom pairs |
| `contract_consistency[].caller_atom` | `string` | Name of the caller atom |
| `contract_consistency[].callee_atom` | `string` | Name of the callee atom |
| `contract_consistency[].is_consistent` | `boolean` | Whether the caller/callee contract pair is consistent |
| `contract_consistency[].violations` | `array[string]` | Contract violations |
| `contract_consistency[].warnings` | `array[string]` | Non-fatal consistency warnings |
| `global_invariants` | `array` | Invariants inferred from repeated atom postconditions |
| `global_invariants[].invariant` | `string` | Invariant expression |
| `global_invariants[].source_atoms` | `array[string]` | Atoms contributing the invariant |
| `global_invariants[].confidence` | `number` | Ratio of contributing atoms to total atoms |
| `circular_dependencies` | `array[array[string]]` | Detected atom dependency cycles |
| `dependency_graph` | `array` | Dependency graph nodes |
| `dependency_graph[].atom_name` | `string` | Atom name |
| `dependency_graph[].dependencies` | `array[string]` | Atoms this atom depends on |
| `dependency_graph[].dependents` | `array[string]` | Atoms that depend on this atom |
| `summary` | `object` | Cross-specification summary |
| `summary.total_atoms` | `number` | Total number of atoms |
| `summary.consistent_calls` | `number` | Number of consistent contract calls |
| `summary.inconsistent_calls` | `number` | Number of inconsistent contract calls |
| `summary.circular_dependency_count` | `number` | Number of detected circular dependencies |
| `summary.global_invariant_count` | `number` | Number of inferred global invariants |

Example:

```json
{
  "contract_consistency": [
    {
      "caller_atom": "transfer",
      "callee_atom": "validate_balance",
      "is_consistent": true,
      "violations": [],
      "warnings": []
    }
  ],
  "global_invariants": [
    {
      "invariant": "balance >= 0",
      "source_atoms": ["transfer", "withdraw"],
      "confidence": 0.5
    }
  ],
  "circular_dependencies": [],
  "dependency_graph": [
    {
      "atom_name": "transfer",
      "dependencies": ["validate_balance"],
      "dependents": []
    }
  ],
  "summary": {
    "total_atoms": 2,
    "consistent_calls": 1,
    "inconsistent_calls": 0,
    "circular_dependency_count": 0,
    "global_invariant_count": 1
  }
}
```
