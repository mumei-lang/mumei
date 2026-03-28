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
  "ensures": "result >= x"
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
