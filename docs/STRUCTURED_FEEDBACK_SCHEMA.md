# Structured Feedback JSON Schema

Mumei verifier feedback uses the `p9-de/v1` loss-vector schema so AI repair
loops can consume counterexamples deterministically.

## Top-level feedback object

```json
{
  "status": "verification_failed",
  "error_type": "postcondition_violated",
  "location": {"file": "src/example.mm", "line": 12},
  "reconstruction_loss": {
    "schema_version": "p9-de/v1",
    "formalization": {
      "specification_space": {
        "symbol": "S",
        "definition": "The contract/specification space induced by requires/ensures clauses."
      },
      "implementation_space": {
        "symbol": "V",
        "definition": "The verified implementation space induced by the atom body and path constraints."
      },
      "metric": "L_recon(S,V,c) = ||eval_S(c) - eval_V(c)|| over the Z3 counterexample c.",
      "zero_loss_condition": "L_recon = 0 iff no counterexample component has non-zero magnitude."
    },
    "violated_property": "result >= 0",
    "counter_example": {"x": -3},
    "loss_vector": [3.0],
    "loss_components": [
      {"variable": "x", "observed": -3, "magnitude": 3.0}
    ]
  },
  "feedback_instruction": "The `ensures` clause is not satisfied for inputs: x=-3."
}
```

## Required fields

| Field | Meaning |
| --- | --- |
| `status` | `verification_failed` or `verification_passed`. |
| `error_type` | Machine-readable failure type; nullable on success. |
| `location` | Source location object `{file,line}`; nullable when unavailable. |
| `reconstruction_loss` | Nullable `p9-de/v1` loss vector payload. |
| `feedback_instruction` | Actionable repair guidance for agents. |

## Reconstruction loss payload

`L_recon(S,V,c)` measures the reconstruction gap between:

- `S`: specification space from contracts (`requires`, `ensures`, invariants).
- `V`: implementation space from atom body/path constraints.
- `c`: Z3 counterexample assignment.

`loss_vector` is ordered by counterexample key. `loss_components` preserves the
same magnitudes with the originating variable name and observed value so agents
can choose targeted repairs instead of rewriting unrelated code.

Zero loss is valid only when every component magnitude is `0` (within solver
epsilon) or the vector is empty.

## Versioning

Schema versions are stable strings under `schema_version`.

- `p9-de/v1`: current loss vector shape.
- Additive fields are allowed in `p9-de/v1`.
- Required-field changes must use a new version such as `p9-de/v2`.

Consumers should ignore unknown fields and route unsupported
`schema_version` values to manual review instead of weakening proofs.
