# Contract Erosion

`mumei erosion` compares the contracts of a candidate `.mm` file with a
baseline version. It reports when a candidate has weakened a baseline
precondition or postcondition, while distinguishing equivalent, stronger, and
refined contracts. Trusted baseline atoms are included because their declared
contracts still describe the obligations being compared. Implementation
methods are reported by qualified name.

The candidate is loaded for comparison, while the baseline is parsed without
resolving its imports. Contract text that is unchanged after whitespace
normalization is preserved directly; normalization keeps token boundaries.
Other contracts are compared in both directions. Changes to clause modes,
callback contracts, or effect-state contracts are reported as inconclusive
rather than assumed equivalent. The command is read-only and does not write
cache or report files.

## Usage

```bash
mumei erosion candidate.mm --baseline baseline.mm --json
```

Without `--json`, the command prints a short human summary. JSON mode writes
the complete report to standard output.

## Statuses

Each baseline atom has one status:

| Status | Meaning |
|---|---|
| `preserved` | The contract text is unchanged or both contract directions are proven. |
| `strengthened` | The candidate contract is stronger than the baseline. |
| `refined` | The candidate refines the baseline, but the reverse implication could not be decided. |
| `pre_weakened` | The candidate requires more than the baseline. |
| `post_weakened` | The candidate does not establish a baseline postcondition. |
| `missing` | No matching candidate atom exists. |
| `signature_mismatch` | A matching atom has an incompatible signature or an ambiguous name. |
| `trust_added` | The candidate marks a baseline atom as trusted. |
| `effects_widened` | The candidate declares an effect absent from the baseline. |
| `inconclusive` | Refinement is undecidable, or clause modes, callback contracts, or effect-state contracts differ. |

`pre_weakened`, `post_weakened`, `missing`, `signature_mismatch`,
`trust_added`, `effects_widened`, and `inconclusive` are erosion statuses and
make the report fail. Differences in `clause_modes`, callback contract fields
(`fn_contract_requires` and `fn_contract_ensures`), or the `effect_pre` and
`effect_post` maps are reported as `inconclusive`. `strengthened` and `refined`
are informational and do not fail the report. Candidate atoms that have no
baseline match are listed in `added` and do not affect the result.

## Exit codes

| Code | Meaning |
|---:|---|
| `0` | No erosion was found. |
| `1` | At least one baseline atom is missing, mismatched, weakened, or inconclusive. |
| `4` | An input could not be read or parsed, or the candidate could not be loaded. |

## JSON schema

The report uses schema `mumei.erosion-report/v1`:

```json
{
  "schema": "mumei.erosion-report/v1",
  "baseline": "baseline.mm",
  "candidate": "candidate.mm",
  "status": "fail",
  "atoms": [
    {
      "name": "clamp",
      "status": "post_weakened",
      "detail": "implementation ensures do not establish the charter postcondition",
      "counterexample": {
        "requested": "2",
        "max_dose": "1",
        "result": "2"
      },
      "matched_candidate": "clamp"
    }
  ],
  "added": [],
  "totals": {
    "checked": 1,
    "eroded": 1,
    "by_status": {
      "post_weakened": 1
    }
  }
}
```

`atoms` follow baseline source order and include `counterexample` and
`matched_candidate` only when available. `added` is sorted. `totals.by_status`
is ordered deterministically.

## Example

Given a baseline:

```mumei
trusted atom clamp(requested: i64, max_dose: i64) -> i64
    requires: requested >= 0 && max_dose >= 0;
    ensures: result >= 0 && result <= max_dose && result <= requested;
    body: 0;
```

and a candidate that drops `result <= max_dose`, compare them with:

```bash
mumei erosion candidate.mm --baseline baseline.mm --json
```

The report marks `clamp` as `post_weakened`, includes a counterexample when
available, and exits with code `1`.
