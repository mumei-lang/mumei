# Proof Certificate Format & Verification Flow

> Documentation for P5 — Verified Asset Distribution (Proof Certificate Chain + Package Registry + Verified Import)

## Certificate Format

A proof certificate (`.proof-cert.json`) is a JSON file containing cryptographically verifiable records of atom verification results.

### ProofCertificate (top-level)

```json
{
  "file": "src/math.mm",
  "generated_at": "2026-03-28T10:00:00Z",
  "z3_version": "4.12.2",
  "atoms": [ ... ],
  "package_name": "my_math_lib",
  "package_version": "1.0.0",
  "certificate_hash": "sha256:abcdef...",
  "all_verified": true
}
```

| Field | Type | Description |
|---|---|---|
| `file` | `String` | Source file path relative to project root |
| `generated_at` | `String` | ISO 8601 timestamp of certificate generation |
| `z3_version` | `String` | Z3 solver version used for verification |
| `atoms` | `Vec<AtomCertificate>` | Per-atom verification records |
| `package_name` | `Option<String>` | Package name from `mumei.toml` (if available) |
| `package_version` | `Option<String>` | Package version from `mumei.toml` (if available) |
| `certificate_hash` | `String` | SHA-256 hash of the serialized certificate (excluding this field) |
| `all_verified` | `bool` | `true` if every atom in the certificate passed verification |

### AtomCertificate (per-atom)

```json
{
  "name": "add",
  "content_hash": "sha256:...",
  "status": "proven",
  "proof_hash": "sha256:...",
  "dependencies": ["helper", "validate"],
  "effects": ["Log"],
  "requires": "x >= 0",
  "ensures": "result >= x",
  "translator_version": "mumei-lean-translator-ir-v1",
  "binder_mapping": { "x": "x", "result": "result" },
  "bridge_lemma_hash": "d8d270d6429a3e31c608dc109876df4ec99ee1243796430775a5b0ef18b5ac24",
  "manual_lemma_reason": null,
  "retry_policy_fingerprint": "9bb6d4f2...",
  "attempt_summary": {
    "total_attempts": 2,
    "attempts_by_action_class": { "llm_fix": 1, "lean_escalation": 1 },
    "final_action_class": "lean_escalation"
  },
  "cost_success_metrics": {
    "attempts_to_success": 2,
    "tokens_to_success": 6400,
    "solver_seconds_to_success": 4.25,
    "spec_drift_score": 0.08
  },
  "translator_ir": {
    "sort": "contract_obligation",
    "binders": [
      { "mumei_name": "x", "lean_name": "x", "mumei_type": "i64", "lean_type": "Int", "role": "param" },
      { "mumei_name": "result", "lean_name": "result", "mumei_type": "i64", "lean_type": "Int", "role": "result" }
    ],
    "theorem_goal": "(x >= 0) -> (result >= x)",
    "provenance_span": { "file": "src/math.mm", "line": 1, "col": 1, "len": 3 },
    "lowering_rules": ["type_system_mapping", "contract_lowering"]
  }
}
```

| Field | Type | Description |
|---|---|---|
| `name` | `String` | Atom name |
| `content_hash` | `String` | SHA-256 hash of the atom's source content |
| `status` | `String` | Verification result: `"proven"`, `"failed"`, or `"skipped"` |
| `proof_hash` | `String` | Dependency-aware hash from `compute_proof_hash()` — includes transitive callee signatures |
| `dependencies` | `Vec<String>` | Direct callee atom names from `ModuleEnv.dependency_graph` |
| `effects` | `Vec<String>` | Declared effect names |
| `requires` | `String` | Precondition contract text |
| `ensures` | `String` | Postcondition contract text |
| `z3_result_class` | `String` | Normalized solver class used for Lean routing (`unknown`, `timeout`, `resource_limit`, `unsat`, etc.) |
| `escalation_reason` | `Option<String>` | Reason an obligation is routed to Lean |
| `logic_fragment_tags` | `Vec<String>` | Detected fragments such as arrays, quantifiers, strings, non-linear arithmetic, or temporal effects |
| `translator_version` | `String` | Lean translator contract version. Version mismatch invalidates Lean proof cache acceptance. |
| `binder_mapping` | `HashMap<String, String>` | Mumei witness/result names mapped to generated Lean binder names |
| `bridge_lemma_hash` | `String` | SHA-256-compatible identifier of the bridge lemma set used during lowering |
| `manual_lemma_reason` | `Option<String>` | Reason partial translation must be completed by a manual lemma |
| `translator_ir` | `TranslatorIRMetadata` | Typed intermediate metadata: obligation sort, binders, theorem goal, provenance span, and lowering rules |
| `lean_metadata` | `Option<LeanResultMetadata>` | Lean result metadata emitted by mumei-lean (`status`, theorem name, translator version, bridge lemma hash, proof path, diagnostics) |
| `retry_policy_fingerprint` | `Option<String>` | SHA-256 fingerprint of the retry budget policy used by the agent/healing process |
| `attempt_summary` | `Option<AttemptSummary>` | Total attempts, attempts by action class, and final action class before success or manual review |
| `cost_success_metrics` | `Option<CostSuccessMetrics>` | Attempts, tokens, solver seconds, and spec drift observed before success |

## Retry Budget Policy Schema (P8-G)

Retry budget metadata makes self-healing and Lean escalation auditable. Policies are intentionally fingerprinted and embedded indirectly so certificates can prove which retry boundary governed an atom without storing full agent logs.

```json
{
  "max_attempts": 5,
  "max_tokens": 10000,
  "max_solver_time_ms": 30000,
  "max_semantic_delta": 0.5,
  "action_class_limits": {
    "llm_fix": {
      "max_attempts": 3,
      "max_tokens": 5000,
      "max_lean_escalations": 0
    },
    "lean_escalation": {
      "max_attempts": 1,
      "max_tokens": 5000,
      "max_lean_escalations": 1
    }
  }
}
```

### Structs

```rust
pub struct BudgetPolicy {
    pub max_attempts: u32,
    pub max_tokens: u64,
    pub max_solver_time_ms: u64,
    pub max_semantic_delta: f64,
    pub action_class_limits: HashMap<String, ActionClassLimit>,
}

pub struct ActionClassLimit {
    pub max_attempts: u32,
    pub max_tokens: u64,
    pub max_lean_escalations: u32,
}

pub struct AttemptSummary {
    pub total_attempts: u32,
    pub attempts_by_action_class: HashMap<String, u32>,
    pub final_action_class: String,
}

pub struct CostSuccessMetrics {
    pub attempts_to_success: u32,
    pub tokens_to_success: u64,
    pub solver_seconds_to_success: f64,
    pub spec_drift_score: f64,
}
```

### Semantics

- `max_attempts`, `max_tokens`, and `max_solver_time_ms` stop runaway repair searches.
- `max_semantic_delta` prevents spec weakening from becoming an unreviewed false success.
- `action_class_limits` bounds strategy-specific retries such as `llm_fix`, `effect_fix`, `precondition_strengthening`, `postcondition_fix`, and `lean_escalation`.
- Repeating the same counterexample signature without a new action class yields `manual_review_required`.
- `retry_policy_fingerprint` is computed from canonical JSON serialization with sorted keys.

## Lean Escalation Translator Contract

Mumei emits a typed escalation bundle for Z3 obligations that are outside the decidable fragment (`unknown`, `timeout`, or `resource_limit`) and for trusted atoms that require higher-assurance review. The bridge is intentionally one-way:

```
escalation_bundle.json -> generated Lean source -> .olean/result certificate -> upgraded proof certificate
```

The Mumei compiler never accepts an upgraded `lean_verified` atom unless the atom source hash still matches and the translator contract metadata is current. A mismatched `translator_version` or `bridge_lemma_hash` is reported as stale and requires re-translation/rebuild.

### Type system mapping

| Mumei | Lean 4 lowering |
|---|---|
| `i64` | `Int` plus bridge lemmas for bounded `[-2^63, 2^63)` semantics when overflow-sensitive |
| `bool` | `Bool` |
| `string` / `Str` | `String` plus string/regex bridge lemmas for operations outside Lean's kernel primitives |
| `array` / `[T]` | `List T` plus array-bounds lemmas for total `get!`/index semantics |
| `struct` | Lean `structure` with field-level types and refinements |
| `enum` | Lean `inductive` with constructor-specific fields |

### Refinement type lowering

A Mumei refinement `{v: T | P(v)}` lowers either to a Lean subtype `{v : T // P v}` when the witness is first-class, or to an explicit predicate argument when the witness is only needed in a theorem statement. `translator_ir.binders[*].refinement` records the raw predicate that produced the lowered proof obligation.

### Loop invariant and recursion encoding

`while` / `for` obligations lower to `translator_ir.sort = "loop_invariant"`. The Lean side must encode these obligations using an induction hypothesis or well-founded recursion proof. If the translator cannot identify a measure or invariant shape, it must mark the atom as `manual_lemma_required` with a non-empty `manual_lemma_reason`.

### Semantic gap bridge

The bridge lemma set covers:

- base type coercions and subtype projection
- integer overflow and bounded `i64` arithmetic
- array bounds and total list indexing semantics
- string/regex semantics
- effect-state transition semantics

Unsupported or partially translated syntax must not be treated as success. It is classified as `manual_lemma_required`, and every such atom must carry `manual_lemma_reason`.

## Proof Hash Algorithm

The `proof_hash` field uses the same algorithm as `compute_proof_hash()` in `resolver.rs`:

```
SHA-256(
  name |
  requires |
  ensures |
  body_expr |
  consumed_params.join(",") |
  resources.join(",") |
  effects.join(",") |
  trust_level |
  callee_signatures (transitive) |
  type_predicates
)
```

This ensures that if any callee's contract changes, all dependent atoms' proof hashes change too, triggering re-verification.

## Verification Flow

### Certificate Generation

```
mumei build --emit proof-cert src/math.mm
```

1. Parse source file
2. Run Z3 verification on all atoms
3. Compute `content_hash` (source hash) and `proof_hash` (dependency-aware hash) for each atom
4. Populate `dependencies`, `effects`, `requires`, `ensures` from `ModuleEnv`
5. Compute `certificate_hash` as SHA-256 of the serialized JSON (excluding the hash field)
6. Write `.proof-cert.json` to output directory

### Certificate Verification

```
mumei verify-cert path/to/.proof-cert.json
```

1. Load the certificate file
2. Find the corresponding source file (from `cert.file`)
3. Re-parse the source and compute current content hashes
4. Compare each atom's stored `content_hash` against the current hash
5. Report per-atom status:
   - **proven**: content hash matches — atom unchanged since last verification
   - **changed**: content hash differs — atom has been modified
   - **unproven**: atom exists in cert but could not be matched
   - **missing**: atom exists in source but not in cert
6. Exit code 0 if all atoms are "proven", exit code 1 otherwise

## Import Verification (P5-C)

When importing a module, the resolver checks for a `.proof-cert.json` file:

### resolve_imports_recursive()

For each imported module (via `import` statements):
1. Look for `.proof-cert.json` in the imported module's directory
2. If found: load and verify with `verify_certificate()`
3. For atoms with "proven" status: call `mark_verified()` as normal
4. For atoms with "changed" or "unproven" status: skip `mark_verified()`, print warning
5. If no cert exists: print warning (or error if `--strict-imports`)

### resolve_manifest_dependencies()

Same verification logic applies to all three dependency types:
- **Path dependencies**: `[dependencies] my_lib = { path = "../my_lib" }`
- **Git dependencies**: `[dependencies] my_lib = { git = "https://..." }`
- **Registry dependencies**: `[dependencies] my_lib = "1.0.0"`

### --strict-imports Flag

```
mumei verify --strict-imports src/main.mm
mumei build --strict-imports src/main.mm
```

When active:
- Missing certificates → **hard error** (compilation stops)
- Invalid/stale certificates → **hard error**
- Only fully "proven" certificates pass

### Taint Integration

When an imported atom fails certificate verification:
1. The atom is registered with `TrustLevel::Unverified`
2. The existing taint analysis in `verification.rs` marks values from these atoms as tainted
3. Tainted values propagate through the dependency chain

## Package Registry Integration (P5-B)

### Publishing

```
mumei publish
```

1. Load `mumei.toml`
2. Verify all source files
3. Generate proof certificate
4. Copy package to `~/.mumei/packages/<name>/<version>/`
5. Save `.proof-cert.json` in the package directory
6. Register in `~/.mumei/registry.json` with `cert_path` and `cert_hash`

### Adding Dependencies

```
mumei add my_package --version 1.0.0
```

1. Resolve package from registry
2. If `cert_path` exists in `VersionEntry`: verify the certificate
3. Report verification status (verified/unverified)
4. Add dependency to `mumei.toml`

### VersionEntry Format

```json
{
  "path": "/home/user/.mumei/packages/my_lib/1.0.0",
  "published_at": "unix:1711612800",
  "atom_count": 5,
  "verified": true,
  "cert_path": "/home/user/.mumei/packages/my_lib/1.0.0/.proof-cert.json",
  "cert_hash": "sha256:..."
}
```

The `cert_path` and `cert_hash` fields are optional (`#[serde(default)]`) for backward compatibility with existing registries.

## Related Files

| File | Role |
|---|---|
| `mumei-core/src/proof_cert.rs` | Certificate generation, verification, SHA-256 utilities |
| `mumei-core/src/resolver.rs` | Import resolution with certificate verification |
| `mumei-core/src/registry.rs` | Package registry with certificate metadata |
| `mumei-core/src/emitter.rs` | `EmitTarget::ProofCert` variant |
| `src/main.rs` | CLI commands: `verify-cert`, `--emit proof-cert`, `--strict-imports` |
