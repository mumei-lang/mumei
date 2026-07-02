//! # Resolver モジュール
//!
//! import 宣言を再帰的に処理し、依存モジュールの型・構造体・atom を
//! ModuleEnv に登録する。循環参照の検出も行う。
//!
//! ## 設計方針
//! - Phase 1: ファイルベースの単純な import 解決
//! - Phase 2+: 完全修飾名（FQN）による名前空間分離、ModuleEnv ベースの管理
//!
//! ## 検証キャッシュ
//! インポートされたモジュールの atom は「検証済み」としてマークされ、
//! main.rs での body 再検証がスキップされる。呼び出し時は requires/ensures
//! の契約のみを信頼する（Compositional Verification）。
//!
//! キャッシュファイル (.mumei_cache) にはソースハッシュと検証結果を永続化し、
//! ソースが変更されていなければ再パース・再検証をスキップする。

mod cache;
mod dependencies;
mod imports;
mod metrics;

#[cfg(test)]
use crate::parser::Item;
#[cfg(test)]
use crate::proof_cert;
#[cfg(test)]
use crate::verification::ModuleEnv;
#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::fs;
#[cfg(test)]
use std::path::Path;

#[cfg(test)]
pub(crate) use cache::compute_hash;
#[cfg(test)]
pub(crate) use imports::{
    check_cert_for_atom, mark_dependency_atoms_with_cert, register_imported_items,
    verify_import_certificate, ResolverContext,
};

pub use cache::{
    collect_callees_from_body, compute_atom_hash, compute_contract_hash, compute_proof_hash,
    compute_proof_hash_with_flags, invalidate_dependents, load_build_cache,
    load_verification_cache, migrate_old_cache, save_build_cache, save_verification_cache,
    VerificationCacheEntry,
};
pub use dependencies::{
    resolve_manifest_dependencies, resolve_manifest_dependencies_with_full_options,
    resolve_manifest_dependencies_with_options,
};
pub use imports::{
    resolve_imports, resolve_imports_with_full_options, resolve_imports_with_options,
    resolve_prelude,
};
pub use metrics::{emit_escalation_bundle, LeanEscalationMetrics};
#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{self, TrustLevel};

    /// P1-A: ExternBlock → trusted atom 自動登録テスト
    #[test]
    fn test_register_extern_block_as_trusted_atoms() {
        let source = r#"
extern "Rust" {
    fn json_parse(input: String) -> String;
    fn json_stringify(obj: String) -> String;
}
"#;
        let items = parser::parse_module(source);
        let mut module_env = ModuleEnv::new();

        register_imported_items(&items, None, &mut module_env);

        // extern 関数が trusted atom として登録されていること
        let json_parse = module_env.get_atom("json_parse");
        assert!(
            json_parse.is_some(),
            "json_parse should be registered as atom"
        );
        let atom = json_parse.unwrap();
        assert_eq!(atom.trust_level, TrustLevel::Trusted);
        assert_eq!(atom.params.len(), 1);
        assert_eq!(atom.params[0].type_name, Some("String".to_string()));

        let json_stringify = module_env.get_atom("json_stringify");
        assert!(
            json_stringify.is_some(),
            "json_stringify should be registered as atom"
        );
        assert_eq!(json_stringify.unwrap().trust_level, TrustLevel::Trusted);
    }

    /// P1-A: ExternBlock with alias → FQN 登録テスト
    #[test]
    fn test_register_extern_block_with_alias() {
        let source = r#"
extern "Rust" {
    fn http_get(url: String) -> String;
}
"#;
        let items = parser::parse_module(source);
        let mut module_env = ModuleEnv::new();

        register_imported_items(&items, Some("http"), &mut module_env);

        // 基本名でも FQN でもアクセスできること
        assert!(
            module_env.get_atom("http_get").is_some(),
            "base name should be registered"
        );
        assert!(
            module_env.get_atom("http::http_get").is_some(),
            "FQN should be registered"
        );

        // FQN 版も trusted であること
        let fqn_atom = module_env.get_atom("http::http_get").unwrap();
        assert_eq!(fqn_atom.trust_level, TrustLevel::Trusted);
    }

    /// P1-A: ExternBlock の複数パラメータテスト
    #[test]
    fn test_extern_block_multi_param() {
        let source = r#"
extern "Rust" {
    fn http_post(url: String, body: String) -> String;
}
"#;
        let items = parser::parse_module(source);
        let mut module_env = ModuleEnv::new();

        register_imported_items(&items, None, &mut module_env);

        let atom = module_env.get_atom("http_post").unwrap();
        assert_eq!(atom.params.len(), 2);
        assert_eq!(atom.params[0].name, "url");
        assert_eq!(atom.params[0].type_name, Some("String".to_string()));
        assert_eq!(atom.params[1].name, "body");
        assert_eq!(atom.params[1].type_name, Some("String".to_string()));
        assert_eq!(atom.requires, "true");
        assert_eq!(atom.ensures, "true");
        assert!(atom.body_expr.is_empty());
    }

    /// P1-A: C 言語 ExternBlock テスト
    #[test]
    fn test_register_extern_block_c_language() {
        let source = r#"
extern "C" {
    fn printf(fmt: i64) -> i64;
}
"#;
        let items = parser::parse_module(source);
        let mut module_env = ModuleEnv::new();

        register_imported_items(&items, None, &mut module_env);

        let atom = module_env.get_atom("printf");
        assert!(atom.is_some(), "C extern function should be registered");
        assert_eq!(atom.unwrap().trust_level, TrustLevel::Trusted);
    }

    /// P1-A: 通常 atom + ExternBlock 混合テスト
    #[test]
    fn test_register_mixed_items_with_extern() {
        let source = r#"
atom add(x: i64, y: i64) -> i64
  requires: true;
  ensures: result == x + y;
  body: x + y;

extern "Rust" {
    fn ffi_helper(n: i64) -> i64;
}
"#;
        let items = parser::parse_module(source);
        let mut module_env = ModuleEnv::new();

        register_imported_items(&items, None, &mut module_env);

        // 通常 atom が登録されていること
        let add = module_env.get_atom("add");
        assert!(add.is_some(), "regular atom should be registered");
        assert_eq!(add.unwrap().trust_level, TrustLevel::Verified);

        // extern atom が trusted で登録されていること
        let ffi = module_env.get_atom("ffi_helper");
        assert!(ffi.is_some(), "extern atom should be registered");
        assert_eq!(ffi.unwrap().trust_level, TrustLevel::Trusted);
    }

    /// compute_hash のテスト
    #[test]
    fn test_compute_hash_deterministic() {
        let hash1 = compute_hash("hello world");
        let hash2 = compute_hash("hello world");
        assert_eq!(hash1, hash2, "same input should produce same hash");

        let hash3 = compute_hash("different");
        assert_ne!(
            hash1, hash3,
            "different input should produce different hash"
        );
    }

    // --- Feature 2: Dependency graph and proof hash tests ---

    /// Test collect_callees_from_body extracts function names
    #[test]
    fn test_collect_callees_from_body() {
        let body = "foo(x) + bar(y, z)";
        let callees = collect_callees_from_body(body);
        assert!(callees.contains("foo"), "should find foo");
        assert!(callees.contains("bar"), "should find bar");
        assert!(!callees.contains("x"), "should not include variable x");
    }

    /// Test collect_callees_from_body skips keywords
    #[test]
    fn test_collect_callees_skips_keywords() {
        let body = "if(cond) { while(true) { match(x) { foo(1) } } }";
        let callees = collect_callees_from_body(body);
        assert!(!callees.contains("if"), "should skip keyword 'if'");
        assert!(!callees.contains("while"), "should skip keyword 'while'");
        assert!(!callees.contains("match"), "should skip keyword 'match'");
        assert!(callees.contains("foo"), "should find foo");
    }

    /// Test compute_proof_hash is deterministic
    #[test]
    fn test_compute_proof_hash_deterministic() {
        let source = r#"
atom add(x: i64, y: i64) -> i64
  requires: x >= 0;
  ensures: result == x + y;
  body: x + y;
"#;
        let items = parser::parse_module(source);
        let module_env = ModuleEnv::new();

        for item in &items {
            if let parser::Item::Atom(atom) = item {
                let hash1 = compute_proof_hash(atom, &module_env);
                let hash2 = compute_proof_hash(atom, &module_env);
                assert_eq!(hash1, hash2, "same atom should produce same proof hash");
            }
        }
    }

    #[test]
    fn test_proof_hash_includes_verification_flags() {
        let source = r#"
atom add(x: i64, y: i64) -> i64
  requires: true;
  ensures: result == x + y;
  body: x + y;
"#;
        let module = parser::parse_module(source);
        let mut module_env = ModuleEnv::new();
        for item in &module {
            if let parser::Item::Atom(atom) = item {
                module_env.register_atom(atom);
            }
        }
        let atom = module_env.get_atom("add").unwrap();

        let default_hash = compute_proof_hash(atom, &module_env);
        let vacuity_hash =
            compute_proof_hash_with_flags(atom, &module_env, &["enable_vacuity_check"]);

        assert_ne!(default_hash, vacuity_hash);
    }

    /// Test compute_proof_hash changes when callee signature changes
    #[test]
    fn test_proof_hash_includes_callee_signature() {
        let source = r#"
atom helper(x: i64) -> i64
  requires: x >= 0;
  ensures: result >= 0;
  body: x;

atom caller(n: i64) -> i64
  requires: n >= 0;
  ensures: result >= 0;
  body: helper(n);
"#;
        let items = parser::parse_module(source);
        let mut module_env = ModuleEnv::new();

        // Register atoms and dependencies
        for item in &items {
            if let parser::Item::Atom(atom) = item {
                module_env.register_atom(atom);
                let callees = collect_callees_from_body(&atom.body_expr);
                module_env.register_dependencies(&atom.name, callees);
            }
        }

        // Compute hash for caller
        let caller_atom = module_env.get_atom("caller").unwrap().clone();
        let hash1 = compute_proof_hash(&caller_atom, &module_env);

        // Now change helper's ensures and re-register
        let source2 = r#"
atom helper(x: i64) -> i64
  requires: x >= 0;
  ensures: result >= 1;
  body: x;

atom caller(n: i64) -> i64
  requires: n >= 0;
  ensures: result >= 0;
  body: helper(n);
"#;
        let items2 = parser::parse_module(source2);
        let mut module_env2 = ModuleEnv::new();
        for item in &items2 {
            if let parser::Item::Atom(atom) = item {
                module_env2.register_atom(atom);
                let callees = collect_callees_from_body(&atom.body_expr);
                module_env2.register_dependencies(&atom.name, callees);
            }
        }

        let caller_atom2 = module_env2.get_atom("caller").unwrap().clone();
        let hash2 = compute_proof_hash(&caller_atom2, &module_env2);

        assert_ne!(
            hash1, hash2,
            "proof hash should change when callee ensures changes"
        );
    }

    /// Test dependency graph transitive dependents
    #[test]
    fn test_dependency_graph_transitive_dependents() {
        let mut module_env = ModuleEnv::new();

        // A calls B, B calls C => changing C should affect both A and B
        let mut callees_a = std::collections::HashSet::new();
        callees_a.insert("B".to_string());
        module_env.register_dependencies("A", callees_a);

        let mut callees_b = std::collections::HashSet::new();
        callees_b.insert("C".to_string());
        module_env.register_dependencies("B", callees_b);

        let dependents_of_c = module_env.get_transitive_dependents("C");
        assert!(dependents_of_c.contains("B"), "B directly depends on C");
        assert!(
            dependents_of_c.contains("A"),
            "A transitively depends on C via B"
        );

        let dependents_of_b = module_env.get_transitive_dependents("B");
        assert!(dependents_of_b.contains("A"), "A directly depends on B");
        assert!(!dependents_of_b.contains("C"), "C does not depend on B");
    }

    /// Test verification cache load/save roundtrip
    #[test]
    fn test_verification_cache_roundtrip() {
        let base_dir =
            std::env::temp_dir().join(format!("mumei_test_cache_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&base_dir);
        let base_dir = base_dir.as_path();

        let mut cache = HashMap::new();
        cache.insert(
            "test_atom".to_string(),
            VerificationCacheEntry {
                proof_hash: "abc123".to_string(),
                result: "verified".to_string(),
                dependencies: vec!["dep1".to_string()],
                type_deps: vec!["Nat".to_string()],
                timestamp: "1234567890s".to_string(),
            },
        );

        save_verification_cache(base_dir, &cache);
        let loaded = load_verification_cache(base_dir);

        assert_eq!(loaded.len(), 1);
        assert!(loaded.contains_key("test_atom"));
        let entry = &loaded["test_atom"];
        assert_eq!(entry.proof_hash, "abc123");
        assert_eq!(entry.result, "verified");
        assert_eq!(entry.dependencies, vec!["dep1"]);
        assert_eq!(entry.type_deps, vec!["Nat"]);
        let _ = std::fs::remove_dir_all(base_dir);
    }

    /// Test invalidate_dependents removes transitive entries
    #[test]
    fn test_invalidate_dependents() {
        let mut module_env = ModuleEnv::new();

        // A calls B, B calls C
        let mut callees_a = std::collections::HashSet::new();
        callees_a.insert("B".to_string());
        module_env.register_dependencies("A", callees_a);

        let mut callees_b = std::collections::HashSet::new();
        callees_b.insert("C".to_string());
        module_env.register_dependencies("B", callees_b);

        let mut cache = HashMap::new();
        for name in &["A", "B", "C"] {
            cache.insert(
                name.to_string(),
                VerificationCacheEntry {
                    proof_hash: format!("hash_{}", name),
                    result: "verified".to_string(),
                    dependencies: vec![],
                    type_deps: vec![],
                    timestamp: "0s".to_string(),
                },
            );
        }

        // Invalidate dependents of C
        invalidate_dependents(&mut cache, "C", &module_env);

        // C itself should still be in cache (we only invalidate dependents)
        assert!(cache.contains_key("C"), "C itself should remain");
        // A and B depend on C, so they should be invalidated
        assert!(
            !cache.contains_key("B"),
            "B depends on C and should be invalidated"
        );
        assert!(
            !cache.contains_key("A"),
            "A transitively depends on C and should be invalidated"
        );
    }

    /// Test migrate_old_cache creates new cache directory
    #[test]
    fn test_migrate_old_cache() {
        let base_dir =
            std::env::temp_dir().join(format!("mumei_test_migrate_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&base_dir);
        let base_dir = base_dir.as_path();

        // Create old-style cache
        let old_cache_path = base_dir.join(".mumei_build_cache");
        let mut old_cache = HashMap::new();
        old_cache.insert("my_atom".to_string(), "oldhash123".to_string());
        let json = serde_json::to_string(&old_cache).unwrap();
        std::fs::write(&old_cache_path, json).unwrap();

        // Run migration
        migrate_old_cache(base_dir);

        // Old file should be deleted
        assert!(
            !old_cache_path.exists(),
            "old cache file should be deleted after migration"
        );

        // New cache should exist
        let new_cache = load_verification_cache(base_dir);
        assert!(
            new_cache.contains_key("my_atom"),
            "migrated atom should exist in new cache"
        );
        assert_eq!(new_cache["my_atom"].proof_hash, "oldhash123");
        assert_eq!(new_cache["my_atom"].result, "verified");
        let _ = std::fs::remove_dir_all(base_dir);
    }

    /// P5-C: check_cert_for_atom returns true for "proven" status
    #[test]
    fn test_check_cert_for_atom_proven() {
        let mut results = HashMap::new();
        results.insert("my_atom".to_string(), "proven".to_string());
        assert!(check_cert_for_atom(&results, "my_atom"));
    }

    /// P5-C: check_cert_for_atom returns false for "changed" status
    #[test]
    fn test_check_cert_for_atom_changed() {
        let mut results = HashMap::new();
        results.insert("my_atom".to_string(), "changed".to_string());
        assert!(!check_cert_for_atom(&results, "my_atom"));
    }

    /// P5-C: check_cert_for_atom returns false for "unproven" status
    #[test]
    fn test_check_cert_for_atom_unproven() {
        let mut results = HashMap::new();
        results.insert("my_atom".to_string(), "unproven".to_string());
        assert!(!check_cert_for_atom(&results, "my_atom"));
    }

    /// P5-C: check_cert_for_atom returns false for missing atom
    #[test]
    fn test_check_cert_for_atom_missing() {
        let results = HashMap::new();
        assert!(!check_cert_for_atom(&results, "nonexistent"));
    }

    /// P5-C: mark_dependency_atoms_with_cert verifies atoms with proven cert
    #[test]
    fn test_mark_dependency_atoms_with_cert_verified() {
        let source = r#"
atom add(x: i64) -> i64
  requires true
  ensures result >= 0
{
  x + 1
}
"#;
        let items = parser::parse_module(source);
        let mut module_env = ModuleEnv::new();
        register_imported_items(&items, Some("dep"), &mut module_env);

        let mut cert_results = HashMap::new();
        cert_results.insert("add".to_string(), "proven".to_string());

        mark_dependency_atoms_with_cert(&items, "dep", &Some(cert_results), &mut module_env, false)
            .unwrap();

        // The atom should be marked as verified
        assert!(module_env.is_verified("add"));
        assert!(module_env.is_verified("dep::add"));
    }

    /// P5-C: mark_dependency_atoms_with_cert marks atoms unverified on failed cert
    #[test]
    fn test_mark_dependency_atoms_with_cert_unverified() {
        let source = r#"
atom add(x: i64) -> i64
  requires true
  ensures result >= 0
{
  x + 1
}
"#;
        let items = parser::parse_module(source);
        let mut module_env = ModuleEnv::new();
        register_imported_items(&items, Some("dep"), &mut module_env);

        let mut cert_results = HashMap::new();
        cert_results.insert("add".to_string(), "changed".to_string());

        mark_dependency_atoms_with_cert(&items, "dep", &Some(cert_results), &mut module_env, false)
            .unwrap();

        // The atom should NOT be marked as verified
        assert!(!module_env.is_verified("add"));
    }

    /// P5-C: mark_dependency_atoms_with_cert with None cert (legacy) verifies all
    #[test]
    fn test_mark_dependency_atoms_with_cert_legacy() {
        let source = r#"
atom foo(x: i64) -> i64
  requires true
  ensures true
{
  x
}
"#;
        let items = parser::parse_module(source);
        let mut module_env = ModuleEnv::new();
        register_imported_items(&items, Some("legacy_dep"), &mut module_env);

        mark_dependency_atoms_with_cert(&items, "legacy_dep", &None, &mut module_env, false)
            .unwrap();

        // Legacy behavior: no cert = all verified
        assert!(module_env.is_verified("foo"));
        assert!(module_env.is_verified("legacy_dep::foo"));
    }

    /// P5-C: ResolverContext strict_imports defaults to false
    #[test]
    fn test_resolver_context_strict_imports_default() {
        let ctx = ResolverContext::new();
        assert!(!ctx.strict_imports);
    }

    /// Verify that verify_import_certificate logic collects ImplBlock methods
    /// with qualified names so they match cert entries like "Stack::push".
    #[test]
    fn test_verify_import_cert_collects_impl_block_methods() {
        use crate::proof_cert;

        // Source with an impl block containing a method
        let source = r#"
struct Stack { top: i64 }
impl Stack {
    atom push(self, val: i64) -> i64
      requires true
      ensures result >= 0
    {
      val
    }
}
"#;
        let items = parser::parse_module(source);

        // Replicate the collection logic from verify_import_certificate
        let mut atom_refs: Vec<&parser::Atom> = items
            .iter()
            .filter_map(|item| match item {
                Item::Atom(a) => Some(a),
                _ => None,
            })
            .collect();

        let mut qualified_methods: Vec<parser::Atom> = Vec::new();
        for item in &items {
            if let Item::ImplBlock(ib) = item {
                for method in &ib.methods {
                    let mut qualified = method.clone();
                    qualified.name = format!("{}::{}", ib.struct_name, method.name);
                    qualified_methods.push(qualified);
                }
            }
        }
        for qm in &qualified_methods {
            atom_refs.push(qm);
        }

        // The collected refs should contain "Stack::push"
        let names: Vec<&str> = atom_refs.iter().map(|a| a.name.as_str()).collect();
        assert!(
            names.contains(&"Stack::push"),
            "Expected 'Stack::push' in atom_refs, got: {:?}",
            names
        );

        // Generate a cert from these atoms, then verify it recognizes the method
        let mut cert_results = std::collections::HashMap::new();
        for a in &atom_refs {
            cert_results.insert(
                a.name.clone(),
                ("unsat".to_string(), "verified".to_string()),
            );
        }
        let module_env = crate::resolver::ModuleEnv::new();
        let cert = proof_cert::generate_certificate(
            "test_impl.mm",
            &atom_refs,
            &cert_results,
            &module_env,
            None,
            None,
            None,
        );

        // Verify that the cert contains the qualified method name
        let cert_names: Vec<&str> = cert.atoms.iter().map(|a| a.name.as_str()).collect();
        assert!(
            cert_names.contains(&"Stack::push"),
            "Expected 'Stack::push' in cert atoms, got: {:?}",
            cert_names
        );

        // Now verify_certificate should report "proven" for "Stack::push"
        let results = proof_cert::verify_certificate(&cert, &atom_refs, false);
        let result_map: std::collections::HashMap<String, String> = results.into_iter().collect();
        assert_eq!(
            result_map.get("Stack::push").map(|s| s.as_str()),
            Some("proven"),
            "Expected 'Stack::push' to be 'proven', got: {:?}",
            result_map.get("Stack::push")
        );
    }

    // =============================================================
    // SI-5 Phase 3-C: MUMEI_PROOF_BUNDLE fallback tests
    // =============================================================
    //
    // These tests mutate the process-wide `MUMEI_PROOF_BUNDLE` env var.
    // `std::env::set_var` is not thread-safe, so we serialise access
    // with a mutex to prevent races when cargo runs tests in parallel.

    use std::sync::Mutex;
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// Helper: produce a bundle containing a single std/ module cert.
    fn make_bundle_with_module(
        module_key: &str,
        source_file: &str,
        atoms: &[&parser::Atom],
    ) -> proof_cert::ProofBundle {
        let mut results = HashMap::new();
        for a in atoms {
            results.insert(
                a.name.clone(),
                ("unsat".to_string(), "verified".to_string()),
            );
        }
        let module_env = ModuleEnv::new();
        let cert = proof_cert::generate_certificate(
            source_file,
            atoms,
            &results,
            &module_env,
            Some("mumei-std"),
            Some("0.0.0"),
            None,
        );
        let mut modules = HashMap::new();
        modules.insert(module_key.to_string(), cert);
        proof_cert::ProofBundle {
            bundle_version: "1.0".to_string(),
            generated_at: "2026-04-18T00:00:00Z".to_string(),
            mumei_version: "test".to_string(),
            modules,
            summary: proof_cert::BundleSummary::default(),
        }
    }

    fn write_bundle_to_tempfile(
        bundle: &proof_cert::ProofBundle,
        name: &str,
    ) -> std::path::PathBuf {
        let tmp = std::env::temp_dir().join(name);
        let json = serde_json::to_string_pretty(bundle).unwrap();
        fs::write(&tmp, json).unwrap();
        tmp
    }

    /// 3-C: module_key_from_source strips `.mm` and keeps the `std/` prefix.
    #[test]
    fn test_module_key_from_source_extracts_std_path() {
        let key =
            proof_cert::module_key_from_source(Path::new("/opt/mumei/std/container/safe_list.mm"));
        assert_eq!(key, Some("std/container/safe_list".to_string()));

        let key = proof_cert::module_key_from_source(Path::new("std/core.mm"));
        assert_eq!(key, Some("std/core".to_string()));

        // A path without a `std/` segment returns None — bundles only
        // carry std/ modules.
        let key = proof_cert::module_key_from_source(Path::new("src/main.mm"));
        assert_eq!(key, None);
    }

    /// 3-C: bundle fallback is used when no local cert exists.
    #[test]
    fn test_verify_import_certificate_bundle_fallback() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let source = r#"
atom add(x: i64, y: i64) -> i64
    requires: true;
    ensures: true;
    body: { x + y };
"#;
        let items = parser::parse_module(source);
        let atom_refs: Vec<&parser::Atom> = items
            .iter()
            .filter_map(|i| match i {
                Item::Atom(a) => Some(a),
                _ => None,
            })
            .collect();

        let bundle = make_bundle_with_module("std/dummy_math", "std/dummy_math.mm", &atom_refs);
        let bundle_path = write_bundle_to_tempfile(&bundle, "mumei_test_bundle_ok.json");

        // Ensure no local cert is present.
        let module_dir = std::env::temp_dir().join("mumei_test_ws3_nocert");
        let _ = fs::remove_dir_all(&module_dir);
        fs::create_dir_all(&module_dir).unwrap();
        let source_file = module_dir.join("dummy_math.mm");
        fs::write(&source_file, source).unwrap();

        // Point source_file at something that module_key_from_source can
        // resolve to `std/dummy_math`.
        let std_root = std::env::temp_dir().join("mumei_test_ws3_std/std");
        let _ = fs::remove_dir_all(std_root.parent().unwrap());
        fs::create_dir_all(&std_root).unwrap();
        let logical_source = std_root.join("dummy_math.mm");
        fs::write(&logical_source, source).unwrap();

        std::env::set_var("MUMEI_PROOF_BUNDLE", &bundle_path);
        let result = verify_import_certificate(&std_root, &logical_source, &items, false);
        std::env::remove_var("MUMEI_PROOF_BUNDLE");

        let result = result.expect("bundle fallback should produce cert results");
        assert_eq!(result.get("add").map(|s| s.as_str()), Some("proven"));

        let _ = fs::remove_file(&bundle_path);
        let _ = fs::remove_dir_all(&module_dir);
        let _ = fs::remove_dir_all(std_root.parent().unwrap());
    }

    #[test]
    fn test_verify_import_certificate_ignores_unmatched_stale_local_cert_for_bundle_fallback() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let source = r#"
atom add(x: i64, y: i64) -> i64
    requires: true;
    ensures: true;
    body: { x + y };
"#;
        let items = parser::parse_module(source);
        let atom_refs: Vec<&parser::Atom> = items
            .iter()
            .filter_map(|i| match i {
                Item::Atom(a) => Some(a),
                _ => None,
            })
            .collect();

        let std_root = std::env::temp_dir().join("mumei_test_ws3_unmatched_stale/std");
        let _ = fs::remove_dir_all(std_root.parent().unwrap());
        fs::create_dir_all(&std_root).unwrap();
        let logical_source = std_root.join("dummy_math.mm");
        fs::write(&logical_source, source).unwrap();

        let bundle = make_bundle_with_module("std/dummy_math", "std/dummy_math.mm", &atom_refs);
        let bundle_path =
            write_bundle_to_tempfile(&bundle, "mumei_test_bundle_unmatched_stale.json");

        let mut local_results = HashMap::new();
        local_results.insert(
            "add".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut stale_local_cert = proof_cert::generate_certificate(
            "std/other_module.mm",
            &atom_refs,
            &local_results,
            &module_env,
            None,
            None,
            None,
        );
        stale_local_cert.atoms[0].translator_version = "stale-translator".to_string();
        proof_cert::save_certificate(&stale_local_cert, &std_root.join(".proof-cert.json"))
            .unwrap();

        std::env::set_var("MUMEI_PROOF_BUNDLE", &bundle_path);
        let result = verify_import_certificate(&std_root, &logical_source, &items, false);
        std::env::remove_var("MUMEI_PROOF_BUNDLE");

        let result = result.expect("unmatched local cert should fall through to bundle fallback");
        assert_eq!(result.get("add").map(|s| s.as_str()), Some("proven"));

        let _ = fs::remove_file(&bundle_path);
        let _ = fs::remove_dir_all(std_root.parent().unwrap());
    }

    /// 3-C: missing/invalid MUMEI_PROOF_BUNDLE path simply falls through.
    #[test]
    fn test_verify_import_certificate_bundle_missing() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let source = r#"
atom sub(x: i64, y: i64) -> i64
    requires: true;
    ensures: true;
    body: { x - y };
"#;
        let items = parser::parse_module(source);

        // Point MUMEI_PROOF_BUNDLE at a nonexistent path.
        let bogus = std::env::temp_dir().join("mumei_test_ws3_missing.json");
        let _ = fs::remove_file(&bogus);

        let module_dir = std::env::temp_dir().join("mumei_test_ws3_missing_dir");
        let _ = fs::remove_dir_all(&module_dir);
        fs::create_dir_all(&module_dir).unwrap();
        let source_file = module_dir.join("sub.mm");
        fs::write(&source_file, source).unwrap();

        std::env::set_var("MUMEI_PROOF_BUNDLE", &bogus);
        let result = verify_import_certificate(&module_dir, &source_file, &items, false);
        std::env::remove_var("MUMEI_PROOF_BUNDLE");

        assert!(
            result.is_none(),
            "missing bundle must not synthesise verification results",
        );
        let _ = fs::remove_dir_all(&module_dir);
    }

    /// 3-C: a local cert always wins against the bundle fallback.
    #[test]
    fn test_verify_import_certificate_local_takes_precedence() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let source = r#"
atom mul(x: i64, y: i64) -> i64
    requires: true;
    ensures: true;
    body: { x * y };
"#;
        let items = parser::parse_module(source);
        let atom_refs: Vec<&parser::Atom> = items
            .iter()
            .filter_map(|i| match i {
                Item::Atom(a) => Some(a),
                _ => None,
            })
            .collect();

        // Bundle certifies `mul` as verified.
        let std_root = std::env::temp_dir().join("mumei_test_ws3_prec/std");
        let _ = fs::remove_dir_all(std_root.parent().unwrap());
        fs::create_dir_all(&std_root).unwrap();
        let logical_source = std_root.join("mul_mod.mm");
        fs::write(&logical_source, source).unwrap();

        let bundle = make_bundle_with_module("std/mul_mod", "std/mul_mod.mm", &atom_refs);
        let bundle_path = write_bundle_to_tempfile(&bundle, "mumei_test_bundle_prec.json");

        // Local cert marks `mul` as FAILED so we can detect which one won.
        let mut local_results = HashMap::new();
        local_results.insert("mul".to_string(), ("sat".to_string(), "failed".to_string()));
        let local_env = ModuleEnv::new();
        let local_cert = proof_cert::generate_certificate(
            logical_source.to_str().unwrap(),
            &atom_refs,
            &local_results,
            &local_env,
            None,
            None,
            None,
        );
        let local_cert_path = std_root.join(".proof-cert.json");
        proof_cert::save_certificate(&local_cert, &local_cert_path).unwrap();

        std::env::set_var("MUMEI_PROOF_BUNDLE", &bundle_path);
        let result = verify_import_certificate(&std_root, &logical_source, &items, false);
        std::env::remove_var("MUMEI_PROOF_BUNDLE");

        let result = result.expect("local cert should produce results");
        // Local cert marked mul as failed → status should not be "proven".
        assert_ne!(
            result.get("mul").map(|s| s.as_str()),
            Some("proven"),
            "local cert must take precedence over the bundle",
        );

        let _ = fs::remove_file(&bundle_path);
        let _ = fs::remove_dir_all(std_root.parent().unwrap());
    }

    #[test]
    fn test_verify_import_certificate_rejects_stale_lean_translator_local_cert() {
        let source = r#"
atom hard_lemma(x: i64) -> i64
    requires: true;
    ensures: true;
    body: { x };
"#;
        let items = parser::parse_module(source);
        let atom_refs: Vec<&parser::Atom> = items
            .iter()
            .filter_map(|i| match i {
                Item::Atom(a) => Some(a),
                _ => None,
            })
            .collect();

        let module_dir = std::env::temp_dir().join("mumei_test_stale_lean_local");
        let _ = fs::remove_dir_all(&module_dir);
        fs::create_dir_all(&module_dir).unwrap();
        let source_file = module_dir.join("hard_lemma.mm");
        fs::write(&source_file, source).unwrap();

        let mut results = HashMap::new();
        results.insert(
            "hard_lemma".to_string(),
            ("lean_verified".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert = proof_cert::generate_certificate(
            source_file.to_str().unwrap(),
            &atom_refs,
            &results,
            &module_env,
            None,
            None,
            None,
        );
        cert.atoms[0].translator_version = "stale-translator".to_string();
        proof_cert::save_certificate(&cert, &module_dir.join(".proof-cert.json"))
            .expect("write stale cert");

        let result = verify_import_certificate(&module_dir, &source_file, &items, true)
            .expect("invalid cert should produce unproven results");
        assert_eq!(
            result.get("hard_lemma").map(|s| s.as_str()),
            Some("unproven"),
            "stale translator metadata must reject the cert"
        );

        let _ = fs::remove_dir_all(&module_dir);
    }

    /// PR 2: Bundle fallback honours `allow_lean_verified` opt-in.
    /// A `lean_verified` atom in the bundle is `"unproven"` by default but
    /// `"proven"` when the resolver is opted in.
    #[test]
    fn test_verify_import_certificate_lean_verified_bundle() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let source = r#"
atom hard_lemma(x: i64) -> i64
    requires: true;
    ensures: true;
    body: { x };
"#;
        let items = parser::parse_module(source);
        let atom_refs: Vec<&parser::Atom> = items
            .iter()
            .filter_map(|i| match i {
                Item::Atom(a) => Some(a),
                _ => None,
            })
            .collect();

        // Build a bundle with a lean_verified atom and Lean result metadata.
        let mut results = HashMap::new();
        results.insert(
            "hard_lemma".to_string(),
            ("lean_verified".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert = proof_cert::generate_certificate(
            "std/lean_pilot.mm",
            &atom_refs,
            &results,
            &module_env,
            None,
            None,
            None,
        );
        cert.atoms[0].lean_metadata = Some(proof_cert::LeanResultMetadata {
            status: "lean_verified".to_string(),
            theorem_name: "hard_lemma_correct".to_string(),
            translator_version: crate::verification::LEAN_TRANSLATOR_VERSION.to_string(),
            bridge_lemma_hash: crate::verification::LEAN_BRIDGE_LEMMA_HASH.to_string(),
            proof_path: "Generated/StdLeanPilot.lean".to_string(),
            diagnostics: vec![],
        });
        let mut modules = HashMap::new();
        modules.insert("std/lean_pilot".to_string(), cert);
        let bundle = proof_cert::ProofBundle {
            bundle_version: "1.0".to_string(),
            generated_at: "2026-04-29T00:00:00Z".to_string(),
            mumei_version: "test".to_string(),
            modules,
            summary: proof_cert::BundleSummary::default(),
        };
        let bundle_path = write_bundle_to_tempfile(&bundle, "mumei_test_bundle_lean.json");

        // Stage a logical std/ source file so module_key_from_source resolves.
        let std_root = std::env::temp_dir().join("mumei_test_lean/std");
        let _ = fs::remove_dir_all(std_root.parent().unwrap());
        fs::create_dir_all(&std_root).unwrap();
        let logical_source = std_root.join("lean_pilot.mm");
        fs::write(&logical_source, source).unwrap();

        std::env::set_var("MUMEI_PROOF_BUNDLE", &bundle_path);

        // Default (allow_lean_verified=false): hard_lemma is NOT proven.
        let default_result = verify_import_certificate(&std_root, &logical_source, &items, false)
            .expect("bundle fallback should produce cert results");
        assert_eq!(
            default_result.get("hard_lemma").map(|s| s.as_str()),
            Some("unproven"),
            "lean_verified must be 'unproven' without --allow-lean-verified",
        );

        // Opt-in (allow_lean_verified=true): hard_lemma IS proven.
        let opt_in_result = verify_import_certificate(&std_root, &logical_source, &items, true)
            .expect("bundle fallback should produce cert results");
        assert_eq!(
            opt_in_result.get("hard_lemma").map(|s| s.as_str()),
            Some("proven"),
            "lean_verified must be 'proven' with --allow-lean-verified",
        );

        std::env::remove_var("MUMEI_PROOF_BUNDLE");
        let _ = fs::remove_file(&bundle_path);
        let _ = fs::remove_dir_all(std_root.parent().unwrap());
    }

    /// Task 1-B: `strict_imports` must propagate from
    /// `resolve_manifest_dependencies_with_full_options` into the
    /// `ResolverContext` used for sub-imports inside each path/git/registry
    /// dependency. Before the fix, only `allow_lean_verified` was
    /// forwarded, which silently weakened strict-mode semantics for
    /// transitive imports.
    ///
    /// Setup: top-level project depends on `dep/` (path dep). `dep/main.mm`
    /// imports `../sub/main.mm`. `dep/` ships a valid `.proof-cert.json`
    /// (so the manifest-level cert check passes) but `sub/` does NOT.
    ///
    ///  - With `strict_imports=false`: resolution succeeds with a warning
    ///    (sub-import has no cert; falls back to legacy "trust").
    ///  - With `strict_imports=true`: resolution must error because the
    ///    sub-import cert is missing — this is only true when the fix
    ///    propagates `strict_imports` into the sub-`ResolverContext`.
    #[test]
    fn test_strict_imports_propagated_to_sub_imports() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let tmp = std::env::temp_dir().join("mumei_test_strict_propagation");
        let _ = fs::remove_dir_all(&tmp);
        let dep_dir = tmp.join("dep");
        let sub_dir = tmp.join("sub");
        fs::create_dir_all(&dep_dir).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();

        // Sub-imported module — has an atom but NO proof certificate.
        let sub_source =
            "atom sub_lemma(x: i64) -> i64 requires: true; ensures: true; body: { x };\n";
        fs::write(sub_dir.join("main.mm"), sub_source).unwrap();

        // Direct path-dep — imports the sub module and has its own cert.
        let dep_source = format!(
            "import \"{}\";\natom dep_lemma(x: i64) -> i64 requires: true; ensures: true; body: {{ x }};\n",
            sub_dir.join("main.mm").display()
        );
        let dep_main = dep_dir.join("main.mm");
        fs::write(&dep_main, &dep_source).unwrap();

        let dep_items = parser::parse_module(&dep_source);
        let dep_atom_refs: Vec<&parser::Atom> = dep_items
            .iter()
            .filter_map(|i| match i {
                Item::Atom(a) => Some(a),
                _ => None,
            })
            .collect();

        let mut results = HashMap::new();
        results.insert(
            "dep_lemma".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let dep_cert = proof_cert::generate_certificate(
            dep_main.to_string_lossy().as_ref(),
            &dep_atom_refs,
            &results,
            &module_env,
            None,
            None,
            None,
        );
        proof_cert::save_certificate(&dep_cert, &dep_dir.join(".proof-cert.json"))
            .expect("write dep cert");

        // Build a manifest with a single path dependency on `dep/`.
        let manifest_toml = format!(
            r#"
[package]
name = "root"
version = "0.0.0"

[dependencies]
dep = {{ path = "{}" }}
"#,
            dep_dir.display()
        );
        let manifest: crate::manifest::Manifest =
            toml::from_str(&manifest_toml).expect("parse manifest");

        // strict_imports = false → warning only, resolution succeeds.
        {
            let mut env = ModuleEnv::new();
            let res = resolve_manifest_dependencies_with_full_options(
                &manifest, &tmp, &mut env, /*strict_imports=*/ false,
                /*allow_lean_verified=*/ false,
            );
            assert!(
                res.is_ok(),
                "non-strict resolution must succeed (got {:?})",
                res
            );
        }

        // strict_imports = true → must error because the *sub-import*
        // (under `sub/main.mm`) has no proof certificate. This only fires
        // when `strict_imports` is propagated into the sub-resolver
        // context — which is exactly what Task 1-B's fix enforces.
        {
            let mut env = ModuleEnv::new();
            let res = resolve_manifest_dependencies_with_full_options(
                &manifest, &tmp, &mut env, /*strict_imports=*/ true,
                /*allow_lean_verified=*/ false,
            );
            let err = res.expect_err(
                "strict_imports=true must reject the sub-import that lacks a proof certificate",
            );
            let msg = format!("{}", err);
            assert!(
                msg.contains("Strict imports") && msg.contains("no proof certificate"),
                "expected sub-import strict error, got: {}",
                msg
            );
        }

        let _ = fs::remove_dir_all(&tmp);
    }
}
