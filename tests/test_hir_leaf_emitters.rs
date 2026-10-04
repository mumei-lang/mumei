use mumei_core::emitter::{CHeaderEmitter, Emitter};
use mumei_core::hir::{lower_atom_to_hir, HirAtom};
use mumei_core::parser::ast::{Atom, Effect, Param, RefinedType, Span, TrustLevel};
use mumei_core::verification::ModuleEnv;
use mumei_emit_json::VerifiedJsonEmitter;
use mumei_emit_proofbook::ProofBookEmitter;
use std::collections::HashMap;
use std::path::Path;

fn param(name: &str, type_name: Option<&str>, is_ref: bool, is_ref_mut: bool) -> Param {
    Param {
        name: name.to_string(),
        type_name: type_name.map(str::to_string),
        type_ref: None,
        is_ref,
        is_ref_mut,
        fn_contract_requires: None,
        fn_contract_ensures: None,
    }
}

fn rich_atom() -> Atom {
    let mut negated_effect = Effect::simple("Beta");
    negated_effect.negated = true;
    Atom {
        name: "rich_atom".to_string(),
        type_params: vec![],
        where_bounds: vec![],
        params: vec![
            param("consume a", Some("i64"), false, false),
            param("b", Some("i64"), true, false),
            param("c", Some("i64"), false, true),
            param("plain", None, false, false),
            param("amount", Some("NonNegative"), false, false),
        ],
        trace_id: None,
        spec_metadata: HashMap::new(),
        requires: "(a >= 0 && b >= 0) && forall(i, 0, a, i >= 0)".to_string(),
        clause_labels: vec![],
        clause_modes: vec![],
        covers: vec![],
        forall_constraints: vec![],
        ensures: "result >= a && result >= amount".to_string(),
        body_expr: "a".to_string(),
        consumed_params: vec![],
        resources: vec!["ledger".to_string()],
        is_async: true,
        trust_level: TrustLevel::Trusted,
        max_unroll: None,
        invariant: None,
        effects: vec![
            Effect::simple("Zeta"),
            Effect::simple("Alpha"),
            Effect::simple("Zeta"),
            negated_effect,
        ],
        return_type: Some("NonNegative".to_string()),
        decreases: None,
        span: Span::default(),
        effect_pre: HashMap::from([("Zeta".to_string(), "Ready".to_string())]),
        effect_post: HashMap::from([("Alpha".to_string(), "Done".to_string())]),
    }
}

fn rich_hir() -> (HirAtom, ModuleEnv) {
    let hir = lower_atom_to_hir(&rich_atom());
    let mut module_env = ModuleEnv::new();
    module_env.register_type(&RefinedType {
        name: "NonNegative".to_string(),
        _base_type: "i64".to_string(),
        operand: "v".to_string(),
        predicate_raw: "v >= 0".to_string(),
        unit: None,
        span: Span::default(),
    });
    (hir, module_env)
}

fn unrelated_atom() -> Atom {
    let mut atom = rich_atom();
    atom.name = "unrelated".to_string();
    atom.params.clear();
    atom.requires = "false".to_string();
    atom.ensures = "false".to_string();
    atom.effects.clear();
    atom.effect_pre.clear();
    atom.effect_post.clear();
    atom.resources.clear();
    atom.is_async = false;
    atom.trust_level = TrustLevel::Unverified;
    atom
}

#[test]
fn verified_json_emitter_reads_hir_and_matches_develop_golden() {
    let expected = r#"{
  "name": "rich_atom",
  "params": [
    {
      "name": "consume a",
      "type": "i64",
      "is_ref": false,
      "is_ref_mut": false
    },
    {
      "name": "b",
      "type": "i64",
      "is_ref": true,
      "is_ref_mut": false
    },
    {
      "name": "c",
      "type": "i64",
      "is_ref": false,
      "is_ref_mut": true
    },
    {
      "name": "plain",
      "type": "i64",
      "is_ref": false,
      "is_ref_mut": false
    },
    {
      "name": "amount",
      "type": "NonNegative",
      "is_ref": false,
      "is_ref_mut": false
    }
  ],
  "requires": "(a >= 0 && b >= 0) && forall(i, 0, a, i >= 0)",
  "ensures": "result >= a && result >= amount",
  "effects": [
    "Zeta",
    "Alpha",
    "Zeta",
    "Beta"
  ],
  "return_type": "NonNegative",
  "trust_level": "Trusted"
}"#;
    let (mut hir, module_env) = rich_hir();
    let before = VerifiedJsonEmitter
        .emit(&hir, Path::new("/tmp/rich_atom"), &module_env, &[])
        .unwrap();
    assert_eq!(before[0].data, expected.as_bytes());

    hir.atom = unrelated_atom();
    let after = VerifiedJsonEmitter
        .emit(&hir, Path::new("/tmp/rich_atom"), &module_env, &[])
        .unwrap();
    assert_eq!(after[0].data, before[0].data);
}

#[test]
fn c_header_emitter_reads_hir_and_matches_develop_golden() {
    let expected = r#"#ifndef RICH_ATOM_H
#define RICH_ATOM_H

#include <stdint.h>

/**
 * @brief rich_atom
 * @pre (a >= 0 && b >= 0) && forall(i, 0, a, i >= 0)
 * @post result >= a && result >= amount
 */
extern int64_t rich_atom(int64_t consume a, int64_t b, int64_t c, int64_t plain, int64_t amount);

#endif /* RICH_ATOM_H */
"#;
    let (mut hir, module_env) = rich_hir();
    let before = CHeaderEmitter
        .emit(&hir, Path::new("/tmp/rich_atom"), &module_env, &[])
        .unwrap();
    assert_eq!(before[0].data, expected.as_bytes());

    hir.atom = unrelated_atom();
    let after = CHeaderEmitter
        .emit(&hir, Path::new("/tmp/rich_atom"), &module_env, &[])
        .unwrap();
    assert_eq!(after[0].data, before[0].data);
}

#[test]
fn proof_book_emitter_reads_hir_and_matches_develop_golden() {
    let expected = r#"# Proof Certificate: `rich_atom`

## Metadata

| Field | Value |
|-------|-------|
| **Atom** | `rich_atom` |
| **Trust Level** | `Trusted` |
| **Mumei Version** | `0.6.20` |
| **Content Hash** | `8567d01de94d94ae` |
| **Async** | Yes |

## Signature

```mumei
atom rich_atom(consume a: i64, b: i64, c: i64, plain: i64, amount: NonNegative (= i64)) -> NonNegative
```

## Formal Contracts

### Precondition (`requires`)

```
(a >= 0 && b >= 0) && forall(i, 0, a, i >= 0)
```

### Postcondition (`ensures`)

```
result >= a && result >= amount
```

## Effects

| Effect | Description |
|--------|-------------|
| `Zeta` | Declared effect |
| `Alpha` | Declared effect |
| `Zeta` | Declared effect |
| `Beta` | Declared effect |

## Temporal Contracts

### Pre-state (`effect_pre`)

- `Zeta`: `Ready`

### Post-state (`effect_post`)

- `Alpha`: `Done`

## Verification Status

> **TRUSTED** — Contracts assumed correct (not verified by Z3).

## Resources

- `ledger`

---

*Generated by Mumei Proof-Book Emitter*
"#;
    let (mut hir, module_env) = rich_hir();
    let before = ProofBookEmitter
        .emit(&hir, Path::new("/tmp/rich_atom"), &module_env, &[])
        .unwrap();
    assert_eq!(before[0].data, expected.as_bytes());

    let expected_hash = mumei_core::proof_cert::compute_atom_content_hash_v2(&hir.atom);
    assert_eq!(hir.meta.content_hash, expected_hash);
    assert!(std::str::from_utf8(&before[0].data)
        .unwrap()
        .contains(&format!(
            "| **Content Hash** | `{}` |",
            &expected_hash[..16]
        )));

    hir.atom = unrelated_atom();
    let after = ProofBookEmitter
        .emit(&hir, Path::new("/tmp/rich_atom"), &module_env, &[])
        .unwrap();
    assert_eq!(after[0].data, before[0].data);
}
