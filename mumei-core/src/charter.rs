use crate::parser::{self, Atom, Item, TrustLevel};
use crate::verification::contract_view::{contract_view, ContractView};
use crate::verification::spec_validation::{
    assert_clause, assert_negated_clause, assert_parameter_refinements, check_with_deadline,
    seed_env, validation_ctx, ClauseLoweringOutcome, DEFAULT_SPEC_VALIDATION_TIMEOUT_MS,
};
use crate::verification::{atom_requires_bitvector_semantics_in_module, ModuleEnv};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};
use z3::{Config, Context, SatResult, Solver};

pub const LOCK_SCHEMA: &str = "mumei.charter-lock/v1";
pub const REPORT_SCHEMA: &str = "mumei.charter-report/v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CharterLock {
    pub schema: String,
    pub charters: Vec<CharterLockEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CharterLockEntry {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockMismatch {
    pub path: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockReport {
    pub status: String,
    pub mismatches: Vec<LockMismatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CharterAtomReport {
    pub charter: String,
    pub name: String,
    pub status: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counterexample: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CharterReport {
    pub schema: String,
    pub status: String,
    pub atoms: Vec<CharterAtomReport>,
    pub lock: LockReport,
}

#[derive(Debug)]
struct NamedAtom<'a> {
    name: String,
    atom: &'a Atom,
}

#[derive(Debug)]
struct LockEvaluation {
    report: LockReport,
    charter_paths: Vec<String>,
}

enum ImplicationResult {
    Proven,
    Counterexample(BTreeMap<String, String>),
    Inconclusive(String),
}

type RefinementFailure = (String, String, Option<BTreeMap<String, String>>);

pub fn write_lock(root: &Path, charter_paths: &[String], lock_path: &Path) -> Result<(), String> {
    if charter_paths.is_empty() {
        return Err("at least one charter file is required".to_string());
    }
    let root = canonical_root(root)?;

    let mut hashes = BTreeMap::new();
    for path in charter_paths {
        validate_charter_path(path)?;
        let charter_path = canonical_charter_path(&root, path)
            .map_err(|error| format!("could not read charter '{path}': {error}"))?;
        let bytes = fs::read(&charter_path)
            .map_err(|error| format!("could not read charter '{path}': {error}"))?;
        hashes.insert(path.clone(), format!("{:x}", Sha256::digest(bytes)));
    }

    let lock = CharterLock {
        schema: LOCK_SCHEMA.to_string(),
        charters: hashes
            .into_iter()
            .map(|(path, sha256)| CharterLockEntry { path, sha256 })
            .collect(),
    };
    let mut bytes = serde_json::to_vec_pretty(&lock)
        .map_err(|error| format!("could not serialize charter lock: {error}"))?;
    bytes.push(b'\n');

    let path = canonical_lock_path(&root, lock_path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "could not create charter lock directory '{}': {error}",
                parent.display()
            )
        })?;
    }
    fs::write(&path, bytes)
        .map_err(|error| format!("could not write charter lock '{}': {error}", path.display()))
}

pub fn check_charters(
    root: &Path,
    charter_paths: &[String],
    lock_path: &Path,
    implementation: &[Item],
    module_env: &ModuleEnv,
) -> Result<CharterReport, String> {
    check_charters_with_generics(
        root,
        charter_paths,
        lock_path,
        implementation,
        &[],
        module_env,
    )
}

pub fn check_charters_with_generics(
    root: &Path,
    charter_paths: &[String],
    lock_path: &Path,
    implementation: &[Item],
    generic_implementation: &[Item],
    module_env: &ModuleEnv,
) -> Result<CharterReport, String> {
    if charter_paths.is_empty() {
        return Err("at least one charter file is required".to_string());
    }
    for path in charter_paths {
        validate_charter_path(path)?;
    }

    let root = canonical_root(root)?;
    let LockEvaluation {
        report: lock_report,
        charter_paths: pinned_charter_paths,
    } = check_lock(&root, charter_paths, lock_path)?;
    let mut charter_atoms = Vec::new();
    let mut paths = charter_paths.to_vec();
    paths.extend(pinned_charter_paths);
    paths.sort();
    paths.dedup();
    for relative_path in paths {
        if validate_charter_path(&relative_path).is_err() {
            continue;
        }
        let path = match root.join(&relative_path).canonicalize() {
            Ok(path) if path.starts_with(&root) => path,
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "could not resolve charter '{}': {error}",
                    relative_path
                ))
            }
        };
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "could not read charter '{}': {error}",
                    path.display()
                ))
            }
        };
        let source = String::from_utf8(bytes)
            .map_err(|error| format!("charter '{}' is not UTF-8: {error}", path.display()))?;
        let items = parser::parse_module_checked(&source).map_err(|errors| {
            format!(
                "could not parse charter '{}': {}",
                path.display(),
                errors.join("\n")
            )
        })?;
        for item in items {
            if let Item::Atom(atom) = item {
                if atom.trust_level != TrustLevel::Trusted {
                    return Err(format!(
                        "charter atom '{}' in '{}' must be trusted",
                        atom.name, relative_path
                    ));
                }
                charter_atoms.push((relative_path.clone(), atom));
            }
        }
    }

    charter_atoms.sort_by(|(left_path, left), (right_path, right)| {
        left_path
            .cmp(right_path)
            .then_with(|| left.name.cmp(&right.name))
    });
    if charter_atoms.is_empty() && lock_report.status == "ok" {
        return Err("no charter atoms were found".to_string());
    }

    let mut implementation_atoms = implementation_atoms(implementation);
    implementation_atoms.extend(generic_implementation_atoms(generic_implementation));
    let mut atoms: Vec<_> = charter_atoms
        .iter()
        .map(|(path, charter_atom)| {
            check_atom(path, charter_atom, &implementation_atoms, module_env)
        })
        .collect();
    if lock_report.status != "ok" {
        for atom in &mut atoms {
            if atom.status == "ok" {
                atom.status = "unpinned".to_string();
                atom.detail = "checked against an unpinned charter text".to_string();
            }
        }
    }
    let status = if lock_report.status == "ok" && atoms.iter().all(|atom| atom.status == "ok") {
        "pass"
    } else {
        "fail"
    };

    Ok(CharterReport {
        schema: REPORT_SCHEMA.to_string(),
        status: status.to_string(),
        atoms,
        lock: lock_report,
    })
}

fn check_lock(
    root: &Path,
    charter_paths: &[String],
    lock_path: &Path,
) -> Result<LockEvaluation, String> {
    let root = canonical_root(root)?;
    let path = canonical_lock_path(&root, lock_path)?;
    let mut requested = charter_paths.to_vec();
    requested.sort();
    requested.dedup();
    let mut mismatches = requested_charter_mismatches(&root, &requested)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            mismatches.push(LockMismatch {
                path: lock_path.display().to_string(),
                detail: "lock file is missing".to_string(),
            });
            mismatches.sort_by(|left, right| {
                left.path
                    .cmp(&right.path)
                    .then_with(|| left.detail.cmp(&right.detail))
            });
            return Ok(LockEvaluation {
                report: LockReport {
                    status: "mismatch".to_string(),
                    mismatches,
                },
                charter_paths: Vec::new(),
            });
        }
        Err(error) => {
            return Err(format!(
                "could not read charter lock '{}': {error}",
                path.display()
            ))
        }
    };
    let lock: CharterLock = serde_json::from_slice(&bytes)
        .map_err(|error| format!("could not parse charter lock '{}': {error}", path.display()))?;
    if lock.schema != LOCK_SCHEMA {
        return Err(format!(
            "unsupported charter lock schema '{}' in '{}'",
            lock.schema,
            path.display()
        ));
    }

    let mut lock_entries = HashMap::new();
    for entry in &lock.charters {
        if lock_entries
            .insert(entry.path.clone(), &entry.sha256)
            .is_some()
        {
            mismatches.push(LockMismatch {
                path: entry.path.clone(),
                detail: "duplicate path in lock".to_string(),
            });
        }
    }

    for relative_path in &requested {
        if !lock_entries.contains_key(relative_path) {
            mismatches.push(LockMismatch {
                path: relative_path.clone(),
                detail: "charter is not listed in lock".to_string(),
            });
        }
    }

    for entry in &lock.charters {
        if let Err(detail) = validate_charter_path(&entry.path) {
            mismatches.push(LockMismatch {
                path: entry.path.clone(),
                detail,
            });
            continue;
        }
        let charter_path = match root.join(&entry.path).canonicalize() {
            Ok(path) if !path.starts_with(&root) => {
                mismatches.push(LockMismatch {
                    path: entry.path.clone(),
                    detail: "charter resolves outside the project".to_string(),
                });
                continue;
            }
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                mismatches.push(LockMismatch {
                    path: entry.path.clone(),
                    detail: "changed without relock: charter is missing".to_string(),
                });
                continue;
            }
            Err(error) => {
                return Err(format!(
                    "could not resolve charter '{}': {error}",
                    entry.path
                ))
            }
        };
        let charter_bytes = match fs::read(&charter_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                mismatches.push(LockMismatch {
                    path: entry.path.clone(),
                    detail: "changed without relock: charter is missing".to_string(),
                });
                continue;
            }
            Err(error) => {
                return Err(format!(
                    "could not read charter '{}': {error}",
                    charter_path.display()
                ))
            }
        };
        let actual_hash = format!("{:x}", Sha256::digest(charter_bytes));
        if actual_hash != entry.sha256 {
            mismatches.push(LockMismatch {
                path: entry.path.clone(),
                detail: "charter changed without relock".to_string(),
            });
        }
    }

    mismatches.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.detail.cmp(&right.detail))
    });
    mismatches.dedup();
    Ok(LockEvaluation {
        report: LockReport {
            status: if mismatches.is_empty() {
                "ok".to_string()
            } else {
                "mismatch".to_string()
            },
            mismatches,
        },
        charter_paths: lock.charters.into_iter().map(|entry| entry.path).collect(),
    })
}

fn requested_charter_mismatches(
    root: &Path,
    charter_paths: &[String],
) -> Result<Vec<LockMismatch>, String> {
    let mut mismatches = Vec::new();
    for relative_path in charter_paths {
        if let Err(detail) = validate_charter_path(relative_path) {
            mismatches.push(LockMismatch {
                path: relative_path.clone(),
                detail,
            });
            continue;
        }
        match root.join(relative_path).canonicalize() {
            Ok(path) if !path.starts_with(root) => mismatches.push(LockMismatch {
                path: relative_path.clone(),
                detail: "charter resolves outside the project".to_string(),
            }),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "could not resolve charter '{}': {error}",
                    relative_path
                ))
            }
        }
    }
    Ok(mismatches)
}

fn validate_charter_path(path: &str) -> Result<(), String> {
    let parsed = Path::new(path);
    if path.is_empty()
        || parsed.is_absolute()
        || parsed.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "charter path must be relative and stay within the project: '{path}'"
        ));
    }
    Ok(())
}

fn canonical_root(root: &Path) -> Result<PathBuf, String> {
    root.canonicalize().map_err(|error| {
        format!(
            "could not resolve project root '{}': {error}",
            root.display()
        )
    })
}

fn canonical_charter_path(root: &Path, path: &str) -> Result<PathBuf, String> {
    let canonical = root
        .join(path)
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !canonical.starts_with(root) {
        return Err(format!("charter resolves outside the project: '{path}'"));
    }
    Ok(canonical)
}

fn canonical_lock_path(root: &Path, path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    let path = root.join(path);
    let mut missing = Vec::new();
    let mut ancestor = path.as_path();
    loop {
        match ancestor.canonicalize() {
            Ok(mut canonical) => {
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = ancestor.file_name().ok_or_else(|| {
                    format!("could not resolve charter lock path '{}'", path.display())
                })?;
                missing.push(name.to_os_string());
                ancestor = ancestor.parent().ok_or_else(|| {
                    format!("could not resolve charter lock path '{}'", path.display())
                })?;
            }
            Err(error) => {
                return Err(format!(
                    "could not resolve charter lock path '{}': {error}",
                    path.display()
                ))
            }
        }
    }
}

fn implementation_atoms(items: &[Item]) -> Vec<NamedAtom<'_>> {
    let mut atoms = Vec::new();
    for item in items {
        match item {
            Item::Atom(atom) => atoms.push(NamedAtom {
                name: atom.name.clone(),
                atom,
            }),
            Item::ImplBlock(block) => {
                atoms.extend(block.methods.iter().map(|atom| NamedAtom {
                    name: format!("{}::{}", block.struct_name, atom.name),
                    atom,
                }));
            }
            _ => {}
        }
    }
    atoms
}

fn generic_implementation_atoms(items: &[Item]) -> Vec<NamedAtom<'_>> {
    implementation_atoms(items)
        .into_iter()
        .filter(|candidate| !candidate.atom.type_params.is_empty())
        .collect()
}

fn check_atom(
    charter_path: &str,
    charter: &Atom,
    implementation: &[NamedAtom<'_>],
    module_env: &ModuleEnv,
) -> CharterAtomReport {
    let exact: Vec<_> = implementation
        .iter()
        .filter(|candidate| candidate.name == charter.name)
        .collect();
    let matches = if !exact.is_empty() {
        exact
    } else if !charter.name.contains("::") {
        let suffix = format!("::{}", charter.name);
        implementation
            .iter()
            .filter(|candidate| candidate.name.ends_with(&suffix))
            .collect()
    } else {
        Vec::new()
    };

    let candidate = match matches.as_slice() {
        [] => {
            return atom_report(
                charter_path,
                &charter.name,
                "missing",
                "no matching implementation atom",
                None,
            )
        }
        [candidate] => candidate,
        _ => {
            let names = matches
                .iter()
                .map(|candidate| candidate.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return atom_report(
                charter_path,
                &charter.name,
                "signature_mismatch",
                &format!("method name is ambiguous: {names}"),
                None,
            );
        }
    };

    if let Some(detail) = signature_mismatch(charter, candidate.atom) {
        return atom_report(
            charter_path,
            &charter.name,
            "signature_mismatch",
            &detail,
            None,
        );
    }

    match check_refinement(charter, candidate.atom, module_env) {
        Ok(()) => atom_report(
            charter_path,
            &charter.name,
            "ok",
            "implementation refines the charter contract",
            None,
        ),
        Err((status, detail, counterexample)) => atom_report(
            charter_path,
            &charter.name,
            &status,
            &detail,
            counterexample,
        ),
    }
}

fn atom_report(
    charter: &str,
    name: &str,
    status: &str,
    detail: &str,
    counterexample: Option<BTreeMap<String, String>>,
) -> CharterAtomReport {
    CharterAtomReport {
        charter: charter.to_string(),
        name: name.to_string(),
        status: status.to_string(),
        detail: detail.to_string(),
        counterexample,
    }
}

fn signature_mismatch(charter: &Atom, implementation: &Atom) -> Option<String> {
    if charter.type_params != implementation.type_params {
        return Some(format!(
            "type parameter mismatch: expected {:?}, found {:?}",
            charter.type_params, implementation.type_params
        ));
    }
    if charter.params.len() != implementation.params.len() {
        return Some(format!(
            "expected {} parameter(s), found {}",
            charter.params.len(),
            implementation.params.len()
        ));
    }
    for (index, (expected, found)) in charter
        .params
        .iter()
        .zip(&implementation.params)
        .enumerate()
    {
        if expected.name != found.name {
            return Some(format!(
                "parameter {} name mismatch: expected '{}', found '{}'",
                index + 1,
                expected.name,
                found.name
            ));
        }
        if expected.type_name != found.type_name {
            return Some(format!(
                "parameter '{}' type mismatch: expected '{}', found '{}'",
                expected.name,
                expected.type_name.as_deref().unwrap_or("<inferred>"),
                found.type_name.as_deref().unwrap_or("<inferred>")
            ));
        }
        if (expected.consume, expected.is_ref, expected.is_ref_mut)
            != (found.consume, found.is_ref, found.is_ref_mut)
        {
            return Some(format!("parameter '{}' ownership mismatch", expected.name));
        }
    }
    let mut charter_consumed = charter.consumed_params.clone();
    let mut implementation_consumed = implementation.consumed_params.clone();
    charter_consumed.sort();
    implementation_consumed.sort();
    charter_consumed.dedup();
    implementation_consumed.dedup();
    if charter_consumed != implementation_consumed {
        return Some(format!(
            "consumed parameters mismatch: expected {:?}, found {:?}",
            charter_consumed, implementation_consumed
        ));
    }
    let charter_return = charter.return_type.as_deref().unwrap_or("i64");
    let implementation_return = implementation.return_type.as_deref().unwrap_or("i64");
    if charter_return != implementation_return {
        return Some(format!(
            "return type mismatch: expected '{charter_return}', found '{implementation_return}'"
        ));
    }
    None
}

fn check_refinement(
    charter: &Atom,
    implementation: &Atom,
    module_env: &ModuleEnv,
) -> Result<(), RefinementFailure> {
    if (!charter.forall_constraints.is_empty() || !implementation.forall_constraints.is_empty())
        && normalized_forall_constraints(charter) != normalized_forall_constraints(implementation)
    {
        return Err((
            "inconclusive".to_string(),
            "quantified clauses differ".to_string(),
            None,
        ));
    }

    match check_implication(
        charter,
        implementation,
        module_env,
        &charter.requires,
        &implementation.requires,
        "pre",
    ) {
        ImplicationResult::Proven => {}
        ImplicationResult::Counterexample(counterexample) => {
            return Err((
                "pre_weakened".to_string(),
                "implementation requires more than the charter".to_string(),
                Some(counterexample),
            ))
        }
        ImplicationResult::Inconclusive(detail) => {
            return Err(("inconclusive".to_string(), detail, None))
        }
    }

    let public_ensures = contract_view(implementation, ContractView::CallerEnsures);
    let post_antecedent = if charter.requires.trim().is_empty() {
        public_ensures
    } else if public_ensures.trim().is_empty() {
        charter.requires.clone()
    } else {
        format!("({}) && ({})", charter.requires, public_ensures)
    };
    match check_implication(
        charter,
        implementation,
        module_env,
        &post_antecedent,
        &charter.ensures,
        "post",
    ) {
        ImplicationResult::Proven => Ok(()),
        ImplicationResult::Counterexample(counterexample) => Err((
            "post_weakened".to_string(),
            "implementation ensures do not establish the charter postcondition".to_string(),
            Some(counterexample),
        )),
        ImplicationResult::Inconclusive(detail) => Err(("inconclusive".to_string(), detail, None)),
    }
}

fn normalized_forall_constraints(atom: &Atom) -> Vec<String> {
    atom.forall_constraints
        .iter()
        .map(|constraint| {
            format!("{constraint:?}")
                .split_whitespace()
                .collect::<String>()
        })
        .collect()
}

fn check_implication(
    charter: &Atom,
    implementation: &Atom,
    module_env: &ModuleEnv,
    antecedent: &str,
    consequent: &str,
    label: &str,
) -> ImplicationResult {
    let timeout_ms = DEFAULT_SPEC_VALIDATION_TIMEOUT_MS;
    let mut config = Config::new();
    config.set_timeout_msec(timeout_ms);
    let context = Context::new(&config);
    let solver = Solver::new(&context);
    let bitvec_i64 = atom_requires_bitvector_semantics_in_module(charter, module_env)
        || atom_requires_bitvector_semantics_in_module(implementation, module_env);
    let vc = validation_ctx(&context, module_env, charter, false, bitvec_i64, false);
    let mut env = seed_env(&vc, charter);
    if let Err(error) = assert_parameter_refinements(&vc, &solver, charter, module_env, &mut env) {
        return ImplicationResult::Inconclusive(error.to_string());
    }

    for (clause, negated) in [(antecedent, false), (consequent, true)] {
        let lowered = if negated {
            assert_negated_clause(&vc, &solver, &mut env, charter, clause, label)
        } else {
            assert_clause(&vc, &solver, &mut env, charter, clause, label)
        };
        match lowered {
            Ok(ClauseLoweringOutcome::Applied) => {}
            Ok(ClauseLoweringOutcome::Skipped(warning)) => {
                return ImplicationResult::Inconclusive(warning)
            }
            Err(error) => return ImplicationResult::Inconclusive(error.to_string()),
        }
    }

    match check_with_deadline(&solver, &context, timeout_ms) {
        SatResult::Unsat => ImplicationResult::Proven,
        SatResult::Sat => {
            let mut counterexample = BTreeMap::new();
            if let Some(model) = solver.get_model() {
                for param in &charter.params {
                    if let Some(value) = env
                        .get(&param.name)
                        .and_then(|value| model.eval(value, true))
                    {
                        counterexample.insert(param.name.clone(), value.to_string());
                    }
                }
            }
            ImplicationResult::Counterexample(counterexample)
        }
        SatResult::Unknown => ImplicationResult::Inconclusive(format!(
            "Z3 could not decide the {label} refinement: {}",
            solver
                .get_reason_unknown()
                .unwrap_or_else(|| "unknown reason".to_string())
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "mumei_charter_{}_{}",
                std::process::id(),
                nonce
            ));
            fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn parse(source: &str) -> Vec<Item> {
        parser::parse_module_checked(source).expect("parse test source")
    }

    fn run_check(charter_source: &str, implementation_source: &str) -> CharterReport {
        let dir = TestDir::new();
        fs::write(dir.0.join("charter.charter.mm"), charter_source).expect("write charter");
        write_lock(
            &dir.0,
            &["charter.charter.mm".to_string()],
            Path::new("charter.lock"),
        )
        .expect("write lock");
        let implementation = parse(implementation_source);
        let module_env = ModuleEnv::new();
        check_charters(
            &dir.0,
            &["charter.charter.mm".to_string()],
            Path::new("charter.lock"),
            &implementation,
            &module_env,
        )
        .expect("check charter")
    }

    fn report_status(report: &CharterReport) -> &str {
        &report.atoms[0].status
    }

    #[test]
    fn lock_round_trip_is_deterministic() {
        let dir = TestDir::new();
        fs::create_dir_all(dir.0.join("specs")).expect("create specs");
        fs::write(dir.0.join("specs/a.charter.mm"), b"alpha").expect("write a");
        fs::write(dir.0.join("specs/b.charter.mm"), b"beta").expect("write b");
        let paths = vec![
            "specs/b.charter.mm".to_string(),
            "specs/a.charter.mm".to_string(),
        ];

        write_lock(&dir.0, &paths, Path::new("charter.lock")).expect("write lock");
        let first = fs::read(dir.0.join("charter.lock")).expect("read first lock");
        write_lock(&dir.0, &paths, Path::new("charter.lock")).expect("rewrite lock");
        let second = fs::read(dir.0.join("charter.lock")).expect("read second lock");

        assert_eq!(first, second);
        assert!(first.ends_with(b"\n"));
        let lock: CharterLock = serde_json::from_slice(&first).expect("parse lock");
        assert_eq!(lock.charters[0].path, "specs/a.charter.mm");
        assert_eq!(lock.schema, LOCK_SCHEMA);
    }

    #[test]
    fn edited_charter_fails_lock_gate() {
        let dir = TestDir::new();
        fs::write(dir.0.join("core.charter.mm"), b"before").expect("write charter");
        write_lock(
            &dir.0,
            &["core.charter.mm".to_string()],
            Path::new("charter.lock"),
        )
        .expect("write lock");
        fs::write(dir.0.join("core.charter.mm"), b"after").expect("edit charter");
        let report = check_lock(
            &dir.0,
            &["core.charter.mm".to_string()],
            Path::new("charter.lock"),
        )
        .expect("check lock");

        assert_eq!(report.report.status, "mismatch");
        assert!(report.report.mismatches[0]
            .detail
            .contains("changed without relock"));
    }

    #[test]
    fn charter_missing_from_lock_fails_lock_gate() {
        let dir = TestDir::new();
        fs::write(dir.0.join("core.charter.mm"), b"source").expect("write charter");
        let lock = CharterLock {
            schema: LOCK_SCHEMA.to_string(),
            charters: Vec::new(),
        };
        fs::write(
            dir.0.join("charter.lock"),
            serde_json::to_vec(&lock).expect("serialize lock"),
        )
        .expect("write lock");

        let report = check_lock(
            &dir.0,
            &["core.charter.mm".to_string()],
            Path::new("charter.lock"),
        )
        .expect("check lock");
        assert_eq!(report.report.status, "mismatch");
        assert!(report.report.mismatches[0]
            .detail
            .contains("not listed in lock"));
    }

    #[test]
    fn deleted_charter_fails_lock_gate() {
        let dir = TestDir::new();
        fs::write(dir.0.join("core.charter.mm"), b"source").expect("write charter");
        write_lock(
            &dir.0,
            &["core.charter.mm".to_string()],
            Path::new("charter.lock"),
        )
        .expect("write lock");
        fs::remove_file(dir.0.join("core.charter.mm")).expect("delete charter");

        let report = check_lock(
            &dir.0,
            &["core.charter.mm".to_string()],
            Path::new("charter.lock"),
        )
        .expect("check lock");
        assert_eq!(report.report.status, "mismatch");
        assert!(report.report.mismatches[0]
            .detail
            .contains("changed without relock"));
    }

    #[cfg(unix)]
    #[test]
    fn charter_symlink_outside_project_is_rejected() {
        use std::os::unix::fs::symlink;

        let root = TestDir::new();
        let outside = TestDir::new();
        let outside_path = outside.0.join("outside.charter.mm");
        fs::write(&outside_path, b"trusted atom outside() -> i64 body: 0;")
            .expect("write outside charter");
        symlink(&outside_path, root.0.join("linked.charter.mm")).expect("create symlink");
        let paths = vec!["linked.charter.mm".to_string()];

        let error = write_lock(&root.0, &paths, Path::new("charter.lock")).unwrap_err();
        assert!(error.contains("outside the project"));

        let lock = CharterLock {
            schema: LOCK_SCHEMA.to_string(),
            charters: vec![CharterLockEntry {
                path: paths[0].clone(),
                sha256: format!(
                    "{:x}",
                    Sha256::digest(fs::read(&outside_path).expect("read outside charter"))
                ),
            }],
        };
        fs::write(
            root.0.join("charter.lock"),
            serde_json::to_vec(&lock).expect("serialize lock"),
        )
        .expect("write lock");
        let report = check_charters(
            &root.0,
            &paths,
            Path::new("charter.lock"),
            &[],
            &ModuleEnv::new(),
        )
        .expect("check outside symlink");
        assert!(report
            .lock
            .mismatches
            .iter()
            .any(|mismatch| mismatch.detail.contains("outside the project")));
        assert!(report.atoms.is_empty());
    }

    #[test]
    fn crlf_charter_hashes_its_bytes() {
        let dir = TestDir::new();
        let bytes = b"trusted atom f() -> i64\r\nbody: 0;\r\n";
        fs::write(dir.0.join("line_endings.charter.mm"), bytes).expect("write charter");
        write_lock(
            &dir.0,
            &["line_endings.charter.mm".to_string()],
            Path::new("charter.lock"),
        )
        .expect("write lock");
        let lock: CharterLock =
            serde_json::from_slice(&fs::read(dir.0.join("charter.lock")).expect("read lock"))
                .expect("parse lock");

        assert_eq!(
            lock.charters[0].sha256,
            format!("{:x}", Sha256::digest(bytes))
        );
    }

    #[test]
    fn missing_atom_fails_coverage() {
        let report = run_check(
            "trusted atom increment(x: i64) -> i64 body: 0;",
            "atom decrement(x: i64) -> i64 body: 0;",
        );
        assert_eq!(report_status(&report), "missing");
    }

    #[test]
    fn parameter_names_types_arity_and_return_types_must_match() {
        let cases = [
            (
                "trusted atom f(x: i64) -> i64 body: 0;",
                "atom f(y: i64) -> i64 body: 0;",
            ),
            (
                "trusted atom f(x: i64) -> i64 body: 0;",
                "atom f(x: bool) -> i64 body: 0;",
            ),
            (
                "trusted atom f(x: i64) -> i64 body: 0;",
                "atom f(x: i64, y: i64) -> i64 body: 0;",
            ),
            (
                "trusted atom f(ref x: i64) -> i64 body: 0;",
                "atom f(consume x: i64) -> i64 body: 0;",
            ),
            (
                "trusted atom f(x: i64) -> i64 body: 0;",
                "atom f(x: i64) -> i64 consume: x; body: 0;",
            ),
            (
                "trusted atom f(x: i64) -> i64 body: 0;",
                "atom f(x: i64) -> bool body: false;",
            ),
        ];
        for (charter_source, implementation_source) in cases {
            let report = run_check(charter_source, implementation_source);
            assert_eq!(report_status(&report), "signature_mismatch");
        }
    }

    #[test]
    fn resolves_methods_by_type_and_method_name() {
        let report = run_check(
            "trusted atom get(self: Point) -> i64 ensures: result >= 0; body: 0;",
            "impl Point { atom get(self: Point) -> i64 ensures: result >= 0; body: 0; }",
        );
        assert_eq!(report_status(&report), "ok");
    }

    #[test]
    fn equal_contracts_and_stronger_ensures_pass() {
        let equal = run_check(
            "trusted atom f(x: i64) -> i64 requires: x >= 0; ensures: result >= x; body: 0;",
            "atom f(x: i64) -> i64 requires: x >= 0; ensures: result >= x; body: 0;",
        );
        assert_eq!(report_status(&equal), "ok");
        let stronger = run_check(
            "trusted atom f(x: i64) -> i64 ensures: result >= x; body: 0;",
            "atom f(x: i64) -> i64 ensures: result > x; body: 0;",
        );
        assert_eq!(report_status(&stronger), "ok");
    }

    #[test]
    fn weaker_requires_passes_and_stronger_requires_fails() {
        let weaker = run_check(
            "trusted atom f(x: i64) -> i64 requires: x >= 0; body: 0;",
            "atom f(x: i64) -> i64 requires: x >= -1; body: 0;",
        );
        assert_eq!(report_status(&weaker), "ok");
        let stronger = run_check(
            "trusted atom f(x: i64) -> i64 requires: x >= 0; body: 0;",
            "atom f(x: i64) -> i64 requires: x > 0; body: 0;",
        );
        assert_eq!(report_status(&stronger), "pre_weakened");
        assert!(stronger.atoms[0]
            .counterexample
            .as_ref()
            .is_some_and(|values| values.contains_key("x")));
    }

    #[test]
    fn weaker_ensures_fails_with_counterexample() {
        let report = run_check(
            "trusted atom f(x: i64) -> i64 ensures: result >= x + 1; body: 0;",
            "atom f(x: i64) -> i64 ensures: result >= x; body: 0;",
        );
        assert_eq!(report_status(&report), "post_weakened");
        assert!(report.atoms[0]
            .counterexample
            .as_ref()
            .is_some_and(|values| values.contains_key("x")));
    }

    #[test]
    fn ensures_check_does_not_establish_a_caller_postcondition() {
        let report = run_check(
            "trusted atom f(x: i64) -> i64 ensures: result >= x + 1; body: 0;",
            "atom f(x: i64) -> i64 ensures check: result >= x + 1; body: 0;",
        );
        assert_eq!(report_status(&report), "post_weakened");
    }

    #[test]
    fn differing_top_level_forall_constraints_are_inconclusive() {
        let report = run_check(
            "trusted atom f(n: i64) -> i64 requires: forall(i, 0, n, i >= 0); body: 0;",
            "atom f(n: i64) -> i64 requires: forall(i, 0, n, i >= 0) && forall(j, 0, n, j >= 0); body: 0;",
        );
        assert_eq!(report_status(&report), "inconclusive");
        assert_eq!(report.atoms[0].detail, "quantified clauses differ");
    }

    #[test]
    fn generic_implementation_is_not_reported_missing() {
        let report = run_check(
            "trusted atom identity<T>(x: T) -> T body: 0;",
            "atom identity<T>(x: T) -> T body: 0;",
        );
        assert_ne!(report_status(&report), "missing");
    }

    #[test]
    fn unsupported_clause_is_inconclusive() {
        let report = run_check(
            "trusted atom f(x: i64) -> i64 requires: unknown_predicate(x); body: 0;",
            "atom f(x: i64) -> i64 body: 0;",
        );
        assert_eq!(report_status(&report), "inconclusive");
    }

    #[test]
    fn empty_clauses_are_true() {
        let report = run_check(
            "trusted atom f(x: i64) -> i64 body: 0;",
            "atom f(x: i64) -> i64 body: 0;",
        );
        assert_eq!(report_status(&report), "ok");
    }

    #[test]
    fn refined_parameter_types_are_seeded_into_the_solver() {
        let mut module_env = ModuleEnv::new();
        let source_items = parse("type Positive = i64 where v > 0;");
        if let Item::TypeDef(type_def) = &source_items[0] {
            module_env.register_type(type_def);
        }
        let dir = TestDir::new();
        fs::write(
            dir.0.join("charter.charter.mm"),
            "trusted atom f(x: Positive) -> i64 requires: x > 0; body: 0;",
        )
        .expect("write charter");
        write_lock(
            &dir.0,
            &["charter.charter.mm".to_string()],
            Path::new("charter.lock"),
        )
        .expect("write lock");
        let implementation = parse("atom f(x: Positive) -> i64 body: 0;");
        let report = check_charters(
            &dir.0,
            &["charter.charter.mm".to_string()],
            Path::new("charter.lock"),
            &implementation,
            &module_env,
        )
        .expect("check charter");
        assert_eq!(report_status(&report), "ok");
    }

    #[test]
    fn several_atoms_keep_mixed_results() {
        let report = run_check(
            "trusted atom first(x: i64) -> i64 ensures: result > x; body: 0;\n\
             trusted atom second(x: i64) -> i64 ensures: result > x; body: 0;",
            "atom first(x: i64) -> i64 ensures: result > x; body: 0;\n\
             atom second(x: i64) -> i64 ensures: result >= x; body: 0;",
        );
        assert_eq!(report.atoms[0].status, "ok");
        assert_eq!(report.atoms[1].status, "post_weakened");
    }
}
