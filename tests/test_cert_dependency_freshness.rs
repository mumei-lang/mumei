use mumei_core::parser::{self, Item};
use mumei_core::proof_cert::{refresh_certificate_integrity, LeanResultMetadata, ProofCertificate};
use mumei_core::resolver;
use mumei_core::verification::{self, ModuleEnv};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    reports: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "mumei_cert_dependency_{}_{}_{}",
            std::process::id(),
            nonce,
            DIR_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir(&root).expect("create exclusive test directory");
        let home = root.join("home");
        let reports = root.join("reports");
        std::fs::create_dir(&home).expect("create isolated home");
        std::fs::create_dir(&reports).expect("create reports directory");
        Self {
            root,
            home,
            reports,
        }
    }

    fn run(&self, args: &[String]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args(args)
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env_remove("MUMEI_PROOF_BUNDLE")
            .output()
            .expect("run mumei")
    }

    fn run_with_proof_bundle(&self, args: &[String], bundle: &Path, std_path: &Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args(args)
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env("MUMEI_PROOF_BUNDLE", bundle)
            .env("MUMEI_STD_PATH", std_path)
            .output()
            .expect("run mumei with proof bundle")
    }

    fn run_in_directory(&self, dir: &Path, args: &[&str]) -> Output {
        let mut args = args
            .iter()
            .map(|arg| (*arg).to_string())
            .collect::<Vec<_>>();
        if args.first().is_some_and(|arg| arg == "verify")
            && !args.iter().any(|arg| arg == "--report-dir")
        {
            let report = self.reports.join(format!(
                "report-{}",
                DIR_COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir(&report).expect("create unique report directory");
            args.splice(
                1..1,
                ["--report-dir".to_string(), report.display().to_string()],
            );
        }
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args(args)
            .current_dir(dir)
            .env("HOME", &self.home)
            .env_remove("MUMEI_PROOF_BUNDLE")
            .output()
            .expect("run mumei in directory")
    }

    fn verify(&self, source: &Path, extra: &[&str]) -> Output {
        let report = self.reports.join(format!(
            "report-{}",
            DIR_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir(&report).expect("create unique report directory");
        let mut args = vec![
            "verify".to_string(),
            "--report-dir".to_string(),
            report.display().to_string(),
        ];
        args.extend(extra.iter().map(|arg| (*arg).to_string()));
        args.push(source.display().to_string());
        self.run(&args)
    }

    fn certify(&self, source: &Path, certificate: &Path) -> Output {
        self.verify(
            source,
            &[
                "--proof-cert",
                "--output",
                certificate.to_str().expect("certificate path is UTF-8"),
            ],
        )
    }

    fn verify_with_proof_bundle(
        &self,
        source: &Path,
        extra: &[&str],
        bundle: &Path,
        std_path: &Path,
    ) -> Output {
        let report = self.reports.join(format!(
            "report-{}",
            DIR_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir(&report).expect("create unique report directory");
        let mut args = vec![
            "verify".to_string(),
            "--report-dir".to_string(),
            report.display().to_string(),
        ];
        args.extend(extra.iter().map(|arg| (*arg).to_string()));
        args.push(source.display().to_string());
        self.run_with_proof_bundle(&args, bundle, std_path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_success(output: &Output, action: &str) {
    assert!(
        output.status.success(),
        "{action} failed with status {:?}\n{}",
        output.status,
        output_text(output)
    );
}

fn setup_modules(fixture: &Fixture) -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {
    let a_dir = fixture.root.join("a");
    let b_dir = fixture.root.join("b");
    let c_dir = fixture.root.join("c");
    std::fs::create_dir(&a_dir).expect("create A directory");
    std::fs::create_dir(&b_dir).expect("create B directory");
    std::fs::create_dir(&c_dir).expect("create C directory");
    let a_source = a_dir.join("a.mm");
    let b_source = b_dir.join("b.mm");
    let c_source = c_dir.join("c.mm");
    let a_cert = a_dir.join(".proof-cert.json");
    let b_cert = b_dir.join(".proof-cert.json");
    std::fs::write(
        &a_source,
        "atom f(x: i64) -> i64\n  requires: x >= 0;\n  ensures: result == x;\n  body: x;\n",
    )
    .expect("write A source");
    std::fs::write(
        &b_source,
        "import \"../a/a.mm\";\natom g(x: i64) -> i64\n  requires: x >= 0;\n  ensures: result == x;\n  body: f(x);\n",
    )
    .expect("write B source");
    std::fs::write(
        &c_source,
        "import \"../b/b.mm\";\natom c(x: i64) -> i64\n  requires: x >= 0;\n  ensures: result == x;\n  body: g(x);\n",
    )
    .expect("write C source");
    (a_source, b_source, c_source, a_cert, b_cert)
}

fn resource_freshness_sources(
    fixture: &Fixture,
    invariant_lower_bound: i64,
    declared: bool,
) -> (PathBuf, PathBuf) {
    let a_source = fixture.root.join("a.mm");
    let b_source = fixture.root.join("b.mm");
    std::fs::write(
        &b_source,
        format!(
            "resource counter {{ value: i64 }} priority: 1 mode: exclusive invariant: counter.value >= {invariant_lower_bound};\n"
        ),
    )
    .expect("write resource definition");
    let resource_clause = if declared {
        "resources: [counter];\n"
    } else {
        ""
    };
    std::fs::write(
        &a_source,
        format!(
            "import \"./b.mm\";\n\
             atom main() -> i64\n\
             {resource_clause}\
             requires: true;\n\
             ensures: true;\n\
             body: {{ acquire counter {{ counter.value = 3; 0 }} }};\n"
        ),
    )
    .expect("write resource caller");
    (a_source, b_source)
}

fn module_resource_freshness_sources(
    fixture: &Fixture,
    invariant_lower_bound: i64,
) -> (PathBuf, PathBuf) {
    let a_source = fixture.root.join("a.mm");
    let b_source = fixture.root.join("b.mm");
    std::fs::write(
        &b_source,
        format!(
            "resource counter {{ value: i64 }} priority: 1 mode: exclusive invariant: counter.value >= {invariant_lower_bound};\n"
        ),
    )
    .expect("write module resource definition");
    std::fs::write(
        &a_source,
        "import \"./b.mm\";\n\
         atom g(a: i64) -> i64\n\
         requires: true;\n\
         ensures: true;\n\
         body: a;\n",
    )
    .expect("write unrelated atom");
    (a_source, b_source)
}

fn verify_cert(fixture: &Fixture, certificate: &Path, source: &Path, strict: bool) -> Output {
    let mut args = vec![
        "verify-cert".to_string(),
        certificate.display().to_string(),
        source.display().to_string(),
    ];
    if strict {
        args.push("--strict".to_string());
    }
    fixture.run(&args)
}

fn verify_cert_with_lean_opt_in(
    fixture: &Fixture,
    certificate: &Path,
    source: &Path,
    strict: bool,
) -> Output {
    let mut args = vec![
        "verify-cert".to_string(),
        certificate.display().to_string(),
        source.display().to_string(),
        "--allow-lean-verified".to_string(),
    ];
    if strict {
        args.push("--strict".to_string());
    }
    fixture.run(&args)
}

fn effect_dependency_source(transition: &str) -> String {
    format!(
        "effect Work\n\
         states: [Idle, Busy];\n\
         initial: Idle;\n\
         transition {transition};\n\
         atom f(x: i64) -> i64\n\
         effects: [Work];\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n\
         atom g(x: i64) -> i64\n\
         effects: [Work];\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: f(x);\n"
    )
}

#[test]
fn imported_enum_signature_change_is_a_dependency_change() {
    let fixture = Fixture::new();
    let source = fixture.root.join("a.mm");
    let imported = fixture.root.join("b.mm");
    let certificate = fixture.root.join("a.proof.json");
    std::fs::write(&imported, "enum E { A(v: i64), B }\n").expect("write enum module");
    std::fs::write(
        &source,
        "import \"./b.mm\";\n\
         atom choose(e: E) -> i64\n\
         requires: true;\n\
         ensures: true;\n\
         body: match e { E::A(v) => v, _ => 0 };\n",
    )
    .expect("write importing module");
    assert_success(
        &fixture.certify(&source, &certificate),
        "certify enum signature",
    );

    let cert: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&certificate).unwrap()).expect("parse certificate");
    let choose = cert
        .atoms
        .iter()
        .find(|atom| atom.name == "choose")
        .expect("choose certificate");
    let dependency_hashes = choose
        .dependency_hashes
        .as_ref()
        .expect("generated dependency hashes");
    assert!(dependency_hashes.contains_key("enum:E"));

    let unchanged = verify_cert(&fixture, &certificate, &source, false);
    let unchanged_text = output_text(&unchanged);
    assert_eq!(unchanged.status.code(), Some(0), "{unchanged_text}");
    assert!(
        unchanged_text.contains("choose: proven"),
        "{unchanged_text}"
    );

    std::fs::write(&imported, "enum E { A(v: i64), B, C }\n").expect("add an enum variant");
    let changed = verify_cert(&fixture, &certificate, &source, false);
    let changed_text = output_text(&changed);
    assert_eq!(changed.status.code(), Some(0), "{changed_text}");
    assert!(
        changed_text.contains("choose: dependency_changed"),
        "{changed_text}"
    );
    assert!(changed_text.contains("enum:E"), "{changed_text}");
}

#[test]
fn imported_struct_enum_field_change_is_a_dependency_change() {
    let fixture = Fixture::new();
    let source = fixture.root.join("a.mm");
    let imported = fixture.root.join("b.mm");
    let certificate = fixture.root.join("a.proof.json");
    std::fs::write(
        &imported,
        "enum E { A(v: i64), B }\nstruct Wrapper { value: E }\n",
    )
    .expect("write enum and struct module");
    std::fs::write(
        &source,
        "import \"./b.mm\";\n\
         atom read(value: Wrapper) -> i64\n\
         requires: true;\n\
         ensures: result == 0;\n\
         body: 0;\n",
    )
    .expect("write importing module");
    assert_success(
        &fixture.certify(&source, &certificate),
        "certify imported struct signature",
    );

    let cert: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&certificate).unwrap()).expect("parse certificate");
    let read = cert
        .atoms
        .iter()
        .find(|atom| atom.name == "read")
        .expect("read certificate");
    let dependency_hashes = read
        .dependency_hashes
        .as_ref()
        .expect("generated dependency hashes");
    assert!(dependency_hashes.contains_key("struct:Wrapper"));
    assert!(dependency_hashes.contains_key("enum:E"));

    let unchanged = verify_cert(&fixture, &certificate, &source, false);
    let unchanged_text = output_text(&unchanged);
    assert_eq!(unchanged.status.code(), Some(0), "{unchanged_text}");
    assert!(unchanged_text.contains("read: proven"), "{unchanged_text}");

    std::fs::write(
        &imported,
        "enum E { A(v: i64), B, C }\nstruct Wrapper { value: E }\n",
    )
    .expect("add enum variant in imported module");
    let changed = verify_cert(&fixture, &certificate, &source, false);
    let changed_text = output_text(&changed);
    assert_eq!(changed.status.code(), Some(0), "{changed_text}");
    assert!(
        changed_text.contains("read: dependency_changed"),
        "{changed_text}"
    );
    assert!(changed_text.contains("enum:E"), "{changed_text}");
}

#[test]
fn stale_transitive_contract_is_rejected_and_claimbook_marks_drift() {
    let fixture = Fixture::new();
    let (a_source, b_source, c_source, a_cert, b_cert) = setup_modules(&fixture);

    assert_success(&fixture.certify(&a_source, &a_cert), "certify A");
    assert_success(&fixture.certify(&b_source, &b_cert), "certify B");
    let first_cert: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&b_cert).unwrap()).expect("parse B certificate");
    let hashes = first_cert.atoms[0]
        .dependency_hashes
        .as_ref()
        .expect("generated dependency hashes");
    assert!(hashes.contains_key("atom:f"), "keys: {hashes:?}");
    assert!(
        hashes.keys().all(|key| !key.starts_with("effect:")),
        "effect-free dependencies should not emit effect keys: {hashes:?}"
    );
    let second_cert_path = fixture.root.join("b-second.proof-cert.json");
    assert_success(
        &fixture.certify(&b_source, &second_cert_path),
        "certify B a second time",
    );
    let second_cert: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&second_cert_path).unwrap())
            .expect("parse second B certificate");
    assert_eq!(
        first_cert.atoms[0].dependency_hashes, second_cert.atoms[0].dependency_hashes,
        "dependency hashes must be deterministic"
    );
    let initial_import = fixture.verify(&c_source, &["--strict-imports"]);
    assert_success(&initial_import, "strict import with unchanged dependencies");

    std::fs::write(
        &a_source,
        "atom f(x: i64) -> i64\n  requires: x >= 0;\n  ensures: result >= 0;\n  body: x;\n",
    )
    .expect("weaken A's ensures");
    assert_success(&fixture.certify(&a_source, &a_cert), "re-certify changed A");

    let changed = verify_cert(&fixture, &b_cert, &b_source, false);
    assert_eq!(changed.status.code(), Some(0), "{}", output_text(&changed));
    let changed_text = output_text(&changed);
    assert!(
        changed_text.contains("dependency_changed"),
        "{changed_text}"
    );
    assert!(changed_text.contains("atom:f"), "{changed_text}");
    let changed_strict = verify_cert(&fixture, &b_cert, &b_source, true);
    let changed_strict_text = output_text(&changed_strict);
    assert_eq!(
        changed_strict.status.code(),
        Some(1),
        "{changed_strict_text}"
    );
    assert!(
        changed_strict_text.contains("dependency_changed"),
        "{changed_strict_text}"
    );
    assert!(
        changed_strict_text.contains("atom:f"),
        "{changed_strict_text}"
    );

    let strict_import = fixture.verify(&c_source, &["--strict-imports"]);
    let strict_text = output_text(&strict_import);
    assert!(!strict_import.status.success(), "{strict_text}");
    assert!(strict_text.contains("g"), "{strict_text}");
    assert!(strict_text.contains("dependency_changed"), "{strict_text}");
    assert!(strict_text.contains("atom:f"), "{strict_text}");

    let non_strict_import = fixture.verify(&c_source, &[]);
    let non_strict_text = output_text(&non_strict_import);
    assert_success(
        &non_strict_import,
        "non-strict import with changed dependencies",
    );
    assert!(
        non_strict_text.lines().any(|line| {
            line.contains("atom 'g'")
                && line.contains("dependency_changed")
                && line.contains("atom:f")
        }),
        "{non_strict_text}"
    );

    let claims_dir = fixture.root.join("claims");
    let claims_output = fixture.run(&[
        "claims".to_string(),
        b_source.display().to_string(),
        "--cert".to_string(),
        b_cert.display().to_string(),
        "--format".to_string(),
        "json".to_string(),
        "--out-dir".to_string(),
        claims_dir.display().to_string(),
    ]);
    assert_success(&claims_output, "generate Claim Book");
    let report: Value =
        serde_json::from_slice(&std::fs::read(claims_dir.join("claimbook.json")).unwrap())
            .expect("parse Claim Book");
    let g = report["claims"]
        .as_array()
        .unwrap()
        .iter()
        .find(|claim| claim["atom"] == "g")
        .expect("g claim");
    assert_eq!(g["freshness"], "drifted");
    assert_eq!(
        g["reason"],
        "a dependency's contract changed since the certificate was produced"
    );
    assert_eq!(g["grade"], "G0");
}

#[test]
fn legacy_dependency_hashes_are_unchecked_with_strict_policy_and_warning() {
    let fixture = Fixture::new();
    let (a_source, b_source, c_source, a_cert, b_cert) = setup_modules(&fixture);
    assert_success(&fixture.certify(&a_source, &a_cert), "certify A");
    assert_success(&fixture.certify(&b_source, &b_cert), "certify B");

    let mut cert: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&b_cert).unwrap()).expect("parse B certificate");
    cert.atoms[0].dependency_hashes = None;
    refresh_certificate_integrity(&mut cert);
    std::fs::write(&b_cert, serde_json::to_vec_pretty(&cert).unwrap())
        .expect("write legacy-form certificate");

    let unchecked = verify_cert(&fixture, &b_cert, &b_source, false);
    let unchecked_text = output_text(&unchecked);
    assert_eq!(unchecked.status.code(), Some(0), "{unchecked_text}");
    assert!(
        unchecked_text.contains("dependency_unchecked"),
        "{unchecked_text}"
    );
    let strict = verify_cert(&fixture, &b_cert, &b_source, true);
    let strict_text = output_text(&strict);
    assert_eq!(strict.status.code(), Some(1), "{strict_text}");
    assert!(
        strict_text.contains("dependency_unchecked"),
        "{strict_text}"
    );

    let non_strict_import = fixture.verify(&c_source, &[]);
    let warning = output_text(&non_strict_import);
    assert_success(&non_strict_import, "non-strict legacy import");
    assert!(
        warning.contains("unchecked dependency freshness"),
        "{warning}"
    );
    assert!(warning.contains("accepted"), "{warning}");
    let strict_import = fixture.verify(&c_source, &["--strict-imports"]);
    let strict_text = output_text(&strict_import);
    assert!(!strict_import.status.success(), "{strict_text}");
    assert!(
        strict_text.contains("dependency_unchecked"),
        "{strict_text}"
    );

    let claims_dir = fixture.root.join("legacy-claims");
    let claims = fixture.run(&[
        "claims".to_string(),
        b_source.display().to_string(),
        "--cert".to_string(),
        b_cert.display().to_string(),
        "--format".to_string(),
        "json".to_string(),
        "--out-dir".to_string(),
        claims_dir.display().to_string(),
    ]);
    assert_success(&claims, "generate Claim Book for legacy certificate");
    let report: Value =
        serde_json::from_slice(&std::fs::read(claims_dir.join("claimbook.json")).unwrap())
            .expect("parse legacy Claim Book");
    let g = report["claims"]
        .as_array()
        .unwrap()
        .iter()
        .find(|claim| claim["atom"] == "g")
        .expect("g claim");
    assert_eq!(g["freshness"], "fresh");
    assert_eq!(g["grade"], "G4");
}

#[test]
fn effect_definition_drift_is_non_strict_warning_and_strict_failure() {
    let fixture = Fixture::new();
    let source = fixture.root.join("effectful.mm");
    let certificate = fixture.root.join(".effectful.proof-cert.json");
    std::fs::write(&source, effect_dependency_source("start: Idle -> Busy"))
        .expect("write effectful source");
    assert_success(
        &fixture.certify(&source, &certificate),
        "certify effectful source",
    );

    let cert: ProofCertificate = serde_json::from_slice(&std::fs::read(&certificate).unwrap())
        .expect("parse effectful certificate");
    let g = cert
        .atoms
        .iter()
        .find(|atom| atom.name == "g")
        .expect("g certificate");
    assert!(g
        .dependency_hashes
        .as_ref()
        .unwrap()
        .contains_key("effect:Work"));

    std::fs::write(&source, effect_dependency_source("start: Idle -> Idle"))
        .expect("change only the effect definition");
    let changed = verify_cert(&fixture, &certificate, &source, false);
    let changed_text = output_text(&changed);
    assert_eq!(changed.status.code(), Some(0), "{changed_text}");
    assert!(
        changed_text.contains("g: dependency_changed"),
        "{changed_text}"
    );
    assert!(changed_text.contains("effect:Work"), "{changed_text}");

    let strict = verify_cert(&fixture, &certificate, &source, true);
    let strict_text = output_text(&strict);
    assert_eq!(strict.status.code(), Some(1), "{strict_text}");
    assert!(
        strict_text.contains("g: dependency_changed"),
        "{strict_text}"
    );
    assert!(strict_text.contains("effect:Work"), "{strict_text}");
}

#[test]
fn legacy_effect_dependency_hashes_are_unchecked_with_strict_policy() {
    let fixture = Fixture::new();
    let source = fixture.root.join("effectful.mm");
    let certificate = fixture.root.join(".effectful.proof-cert.json");
    std::fs::write(&source, effect_dependency_source("start: Idle -> Busy"))
        .expect("write effectful source");
    assert_success(
        &fixture.certify(&source, &certificate),
        "certify effectful source",
    );

    let mut cert: ProofCertificate = serde_json::from_slice(&std::fs::read(&certificate).unwrap())
        .expect("parse effectful certificate");
    for atom in &mut cert.atoms {
        if let Some(dependency_hashes) = &mut atom.dependency_hashes {
            dependency_hashes.retain(|key, _| !key.starts_with("effect:"));
        }
    }
    refresh_certificate_integrity(&mut cert);
    std::fs::write(&certificate, serde_json::to_vec_pretty(&cert).unwrap())
        .expect("write certificate without effect hashes");

    let unchecked = verify_cert(&fixture, &certificate, &source, false);
    let unchecked_text = output_text(&unchecked);
    assert_eq!(unchecked.status.code(), Some(0), "{unchecked_text}");
    assert!(
        unchecked_text.contains("g: dependency_unchecked"),
        "{unchecked_text}"
    );
    assert!(unchecked_text.contains("effect:Work"), "{unchecked_text}");

    let strict = verify_cert(&fixture, &certificate, &source, true);
    let strict_text = output_text(&strict);
    assert_eq!(strict.status.code(), Some(1), "{strict_text}");
    assert!(
        strict_text.contains("g: dependency_unchecked"),
        "{strict_text}"
    );
    assert!(strict_text.contains("effect:Work"), "{strict_text}");
}

#[test]
fn changed_requires_and_refined_types_are_dependency_changes() {
    let fixture = Fixture::new();
    let (a_source, b_source, _, a_cert, b_cert) = setup_modules(&fixture);
    std::fs::write(
        &a_source,
        "atom f(x: i64) -> i64\n  requires: true;\n  ensures: result == x;\n  body: x;\n",
    )
    .expect("write original A requires");
    assert_success(&fixture.certify(&a_source, &a_cert), "certify A");
    assert_success(&fixture.certify(&b_source, &b_cert), "certify B");
    std::fs::write(
        &a_source,
        "atom f(x: i64) -> i64\n  requires: x >= 0;\n  ensures: result == x;\n  body: x;\n",
    )
    .expect("strengthen A's requires");
    assert_success(
        &fixture.certify(&a_source, &a_cert),
        "re-certify A with stronger requires",
    );
    let changed_requires = verify_cert(&fixture, &b_cert, &b_source, false);
    assert_eq!(
        changed_requires.status.code(),
        Some(0),
        "{}",
        output_text(&changed_requires)
    );
    let requires_text = output_text(&changed_requires);
    assert!(
        requires_text.contains("dependency_changed"),
        "{requires_text}"
    );
    assert!(requires_text.contains("atom:f"), "{requires_text}");

    let type_fixture = Fixture::new();
    let source = type_fixture.root.join("typed.mm");
    let certificate = type_fixture.root.join(".proof-cert.json");
    std::fs::write(
        &source,
        "type Positive = i64 where v >= 0;\natom g(x: Positive) -> Positive\n  requires: true;\n  ensures: result >= 0;\n  body: x;\n",
    )
    .expect("write refined-type source");
    assert_success(
        &type_fixture.certify(&source, &certificate),
        "certify refined type",
    );
    std::fs::write(
        &source,
        "type Positive = i64 where v > 0;\natom g(x: Positive) -> Positive\n  requires: true;\n  ensures: result >= 0;\n  body: x;\n",
    )
    .expect("change refined-type predicate");
    let changed_type = verify_cert(&type_fixture, &certificate, &source, false);
    let type_text = output_text(&changed_type);
    assert!(changed_type.status.success(), "{type_text}");
    assert!(type_text.contains("dependency_changed"), "{type_text}");
    assert!(type_text.contains("type:Positive"), "{type_text}");
}

#[test]
fn imported_unit_tag_change_is_a_dependency_change() {
    let fixture = Fixture::new();
    let (a_source, b_source, _, a_cert, b_cert) = setup_modules(&fixture);
    std::fs::write(
        &a_source,
        "type Money = i64 unit USD;\n\
         atom f(x: Money) -> Money\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n",
    )
    .expect("write A with USD type");
    std::fs::write(
        &b_source,
        "import \"../a/a.mm\";\n\
         atom g(x: Money) -> Money\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: f(x);\n",
    )
    .expect("write B using imported Money type");
    assert_success(&fixture.certify(&a_source, &a_cert), "certify A with USD");
    assert_success(&fixture.certify(&b_source, &b_cert), "certify B with USD");

    std::fs::write(
        &a_source,
        "type Money = i64 unit EUR;\n\
         atom f(x: Money) -> Money\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n",
    )
    .expect("change only A's unit tag");
    assert_success(
        &fixture.certify(&a_source, &a_cert),
        "re-certify A with EUR",
    );
    let changed = verify_cert(&fixture, &b_cert, &b_source, false);
    let changed_text = output_text(&changed);
    assert!(
        changed_text
            .lines()
            .any(|line| line.contains("g: dependency_changed")),
        "{changed_text}"
    );
    assert!(changed_text.contains("type:Money"), "{changed_text}");
}

#[test]
fn imported_effect_post_change_is_a_dependency_change() {
    let fixture = Fixture::new();
    let a_dir = fixture.root.join("a");
    let b_dir = fixture.root.join("b");
    std::fs::create_dir(&a_dir).expect("create A directory");
    std::fs::create_dir(&b_dir).expect("create B directory");
    let a_source = a_dir.join("a.mm");
    let b_source = b_dir.join("b.mm");
    let a_cert = a_dir.join(".proof-cert.json");
    let b_cert = b_dir.join(".proof-cert.json");
    let a_with_post = |post: &str| {
        format!(
            "effect File\n\
             states: [Open, Closed];\n\
             initial: Open;\n\
             transition close: Open -> Closed;\n\
             transition open: Closed -> Open;\n\
             trusted atom f(x: i64) -> i64\n\
             effects: [File];\n\
             effect_pre: {{ File: Open }};\n\
             effect_post: {{ File: {post} }};\n\
             requires: x >= 0;\n\
             ensures: result == x;\n\
             body: x;\n"
        )
    };
    std::fs::write(&a_source, a_with_post("Open")).expect("write A with Open post-state");
    std::fs::write(
        &b_source,
        "import \"../a/a.mm\";\n\
         atom g(x: i64) -> i64\n\
         effects: [File];\n\
         effect_pre: { File: Open };\n\
         effect_post: { File: Open };\n\
         requires: x >= 0;\n\
         ensures: result == x;\n\
         body: f(x);\n",
    )
    .expect("write B caller");
    assert_success(
        &fixture.certify(&a_source, &a_cert),
        "certify A with Open post-state",
    );
    assert_success(
        &fixture.certify(&b_source, &b_cert),
        "certify B with Open post-state",
    );

    std::fs::write(&a_source, a_with_post("Closed")).expect("change A's effect_post only");
    assert_success(
        &fixture.certify(&a_source, &a_cert),
        "re-certify A with Closed post-state",
    );
    let changed = verify_cert(&fixture, &b_cert, &b_source, false);
    let changed_text = output_text(&changed);
    assert!(
        changed_text
            .lines()
            .any(|line| line.contains("g: dependency_changed")),
        "{changed_text}"
    );
    assert!(changed_text.contains("atom:f"), "{changed_text}");
}

#[test]
fn lean_verified_dependency_acceptance_requires_fresh_dependencies() {
    let fixture = Fixture::new();
    let (a_source, b_source, _, a_cert, b_cert) = setup_modules(&fixture);
    assert_success(&fixture.certify(&a_source, &a_cert), "certify A");
    assert_success(&fixture.certify(&b_source, &b_cert), "certify B");
    let mut cert: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&b_cert).unwrap()).expect("parse B certificate");
    let atom = &mut cert.atoms[0];
    atom.z3_check_result = "lean_verified".to_string();
    atom.translator_version = verification::LEAN_TRANSLATOR_VERSION.to_string();
    atom.bridge_lemma_hash = verification::LEAN_BRIDGE_LEMMA_HASH.to_string();
    atom.lean_result_metadata = Some(LeanResultMetadata {
        status: "lean_verified".to_string(),
        theorem_name: "g_spec".to_string(),
        translator_version: verification::LEAN_TRANSLATOR_VERSION.to_string(),
        bridge_lemma_hash: verification::LEAN_BRIDGE_LEMMA_HASH.to_string(),
        proof_path: "Generated/Test.lean".to_string(),
        diagnostics: vec![],
        ..Default::default()
    });
    refresh_certificate_integrity(&mut cert);
    std::fs::write(&b_cert, serde_json::to_vec_pretty(&cert).unwrap())
        .expect("write Lean certificate");

    let unchanged = fixture.verify(
        &fixture.root.join("c/c.mm"),
        &["--strict-imports", "--allow-lean-verified"],
    );
    let unchanged_text = output_text(&unchanged);
    assert_success(
        &unchanged,
        "verify Lean certificate with fresh dependencies",
    );
    assert!(
        unchanged_text.contains("Lean-verified atom 'g' accepted as proven"),
        "{unchanged_text}"
    );

    std::fs::write(
        &a_source,
        "atom f(x: i64) -> i64\n  requires: x >= 0;\n  ensures: result >= 0;\n  body: x;\n",
    )
    .expect("weaken A's ensures");
    assert_success(&fixture.certify(&a_source, &a_cert), "re-certify changed A");
    let changed = fixture.verify(
        &fixture.root.join("c/c.mm"),
        &["--strict-imports", "--allow-lean-verified"],
    );
    let changed_text = output_text(&changed);
    assert!(!changed.status.success(), "{changed_text}");
    assert!(
        changed_text.contains("dependency_changed"),
        "{changed_text}"
    );
    assert!(
        !changed_text.contains("Lean-verified atom 'g' accepted as proven"),
        "{changed_text}"
    );
}

#[test]
fn lean_callee_trust_policy_does_not_change_dependency_freshness() {
    let fixture = Fixture::new();
    let (a_source, b_source, c_source, a_cert, b_cert) = setup_modules(&fixture);
    assert_success(&fixture.certify(&a_source, &a_cert), "certify A");

    let mut a_certificate: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&a_cert).unwrap()).expect("parse A certificate");
    let f = a_certificate
        .atoms
        .iter_mut()
        .find(|atom| atom.name == "f")
        .expect("f certificate");
    f.z3_check_result = "lean_verified".to_string();
    f.translator_version = verification::LEAN_TRANSLATOR_VERSION.to_string();
    f.bridge_lemma_hash = verification::LEAN_BRIDGE_LEMMA_HASH.to_string();
    f.lean_result_metadata = Some(LeanResultMetadata {
        status: "lean_verified".to_string(),
        theorem_name: "f_spec".to_string(),
        translator_version: verification::LEAN_TRANSLATOR_VERSION.to_string(),
        bridge_lemma_hash: verification::LEAN_BRIDGE_LEMMA_HASH.to_string(),
        proof_path: "Generated/Test.lean".to_string(),
        diagnostics: vec![],
        ..Default::default()
    });
    refresh_certificate_integrity(&mut a_certificate);
    std::fs::write(&a_cert, serde_json::to_vec_pretty(&a_certificate).unwrap())
        .expect("write Lean-verified A certificate");

    let b_certified = fixture.verify(
        &b_source,
        &[
            "--allow-lean-verified",
            "--proof-cert",
            "--output",
            b_cert.to_str().unwrap(),
        ],
    );
    assert_success(&b_certified, "certify B with Lean opt-in");

    let without_opt_in = verify_cert(&fixture, &b_cert, &b_source, false);
    let without_opt_in_text = output_text(&without_opt_in);
    assert_success(&without_opt_in, "verify B without Lean opt-in");
    assert!(
        without_opt_in_text
            .lines()
            .any(|line| line.contains("g: proven")),
        "{without_opt_in_text}"
    );
    assert!(
        without_opt_in_text.lines().any(|line| {
            line.contains("atom 'f'") && line.contains("failed certificate verification")
        }),
        "{without_opt_in_text}"
    );
    assert!(
        !without_opt_in_text
            .lines()
            .any(|line| line.contains("g: dependency_changed")),
        "{without_opt_in_text}"
    );

    let without_opt_in_import = fixture.verify(&c_source, &[]);
    let without_opt_in_import_text = output_text(&without_opt_in_import);
    assert_success(&without_opt_in_import, "import B without Lean opt-in");
    assert!(
        without_opt_in_import_text.lines().any(|line| {
            line.contains("atom 'f'") && line.contains("failed certificate verification")
        }),
        "{without_opt_in_import_text}"
    );
    assert!(
        !without_opt_in_import_text
            .lines()
            .any(|line| line.contains("atom 'g'") && line.contains("dependency_changed")),
        "{without_opt_in_import_text}"
    );

    let with_opt_in = verify_cert_with_lean_opt_in(&fixture, &b_cert, &b_source, true);
    let with_opt_in_text = output_text(&with_opt_in);
    assert_success(&with_opt_in, "strictly verify B with Lean opt-in");
    assert!(
        with_opt_in_text
            .lines()
            .any(|line| line.contains("g: proven")),
        "{with_opt_in_text}"
    );

    let strict_import = fixture.verify(&c_source, &["--strict-imports", "--allow-lean-verified"]);
    assert_success(&strict_import, "strictly import B with Lean opt-in");
}

#[test]
fn generic_enum_dependency_hash_matches_strict_import_path() {
    let fixture = Fixture::new();
    let a_dir = fixture.root.join("a");
    let b_dir = fixture.root.join("b");
    std::fs::create_dir(&a_dir).expect("create A directory");
    std::fs::create_dir(&b_dir).expect("create B directory");
    let a_source = a_dir.join("a.mm");
    let b_source = b_dir.join("b.mm");
    let b_cert = b_dir.join(".proof-cert.json");
    let b_module = "enum Opt<T> { Some(v: T), Nothing }\n\
                    atom get(opt: Opt<i64>) -> i64\n\
                    requires: true;\n\
                    ensures: true;\n\
                    body: 0;\n";
    std::fs::write(&b_source, b_module).expect("write generic enum module");
    std::fs::write(
        &a_source,
        "import \"../b/b.mm\";\n\
         atom main() -> i64\n\
         requires: true;\n\
         ensures: result == 0;\n\
         body: 0;\n",
    )
    .expect("write strict-importing module");

    assert_success(
        &fixture.certify(&b_source, &b_cert),
        "certify generic enum module",
    );
    let cert: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&b_cert).unwrap()).expect("parse B certificate");
    let cert_enum_hash = cert.atoms[0]
        .dependency_hashes
        .as_ref()
        .expect("generated dependency hashes")
        .get("enum:Opt")
        .expect("generic enum dependency hash");

    let parse_time_enum_hash = {
        let mut module_env = ModuleEnv::new();
        let mut atom = None;
        for item in parser::parse_module(b_module) {
            match item {
                Item::Atom(parsed_atom) => {
                    if parsed_atom.name == "get" {
                        atom = Some(parsed_atom.clone());
                    }
                    module_env.register_atom(&parsed_atom);
                }
                Item::EnumDef(enum_def) => module_env.register_enum(&enum_def),
                _ => {}
            }
        }
        let atom = atom.expect("parse generic enum atom");
        resolver::compute_dependency_hashes(&atom, &module_env)
            .remove("enum:Opt")
            .expect("parse-time generic enum dependency hash")
    };
    assert_eq!(
        cert_enum_hash, &parse_time_enum_hash,
        "enum:Opt hash differs between certificate and parse-time import inputs"
    );

    let unchanged_import = fixture.verify(&a_source, &["--strict-imports"]);
    assert!(
        unchanged_import.status.success(),
        "unchanged strict import rejected the certificate; certificate enum:Opt={cert_enum_hash}, \
         parse-time enum:Opt={parse_time_enum_hash}\n{}",
        output_text(&unchanged_import)
    );

    std::fs::write(&b_source, b_module.replace("Nothing }", "Nothing, Extra }"))
        .expect("add generic enum variant without regenerating certificate");
    let changed_import = fixture.verify(&a_source, &["--strict-imports"]);
    let changed_text = output_text(&changed_import);
    assert!(
        !changed_import.status.success(),
        "strict import accepted a changed generic enum without a fresh certificate:\n{changed_text}"
    );
    assert!(
        changed_text.contains("dependency_changed"),
        "{changed_text}"
    );
    assert!(changed_text.contains("enum:Opt"), "{changed_text}");
}

#[test]
fn transitive_dependencies_and_imported_refined_types_are_tracked() {
    let fixture = Fixture::new();
    let a0_dir = fixture.root.join("a0");
    let a1_dir = fixture.root.join("a1");
    let b_dir = fixture.root.join("b");
    std::fs::create_dir(&a0_dir).expect("create A0 directory");
    std::fs::create_dir(&a1_dir).expect("create A1 directory");
    std::fs::create_dir(&b_dir).expect("create B directory");
    let a0_source = a0_dir.join("a0.mm");
    let a1_source = a1_dir.join("a1.mm");
    let b_source = b_dir.join("b.mm");
    let a0_cert = a0_dir.join(".proof-cert.json");
    let a1_cert = a1_dir.join(".proof-cert.json");
    let b_cert = b_dir.join(".proof-cert.json");
    std::fs::write(
        &a0_source,
        "atom f0(x: i64) -> i64\n  requires: x >= 0;\n  ensures: result >= x;\n  body: x;\n",
    )
    .expect("write A0 source");
    std::fs::write(
        &a1_source,
        "import \"../a0/a0.mm\";\natom f1(x: i64) -> i64\n  requires: x >= 0;\n  ensures: result >= 0;\n  body: f0(x);\n",
    )
    .expect("write A1 source");
    std::fs::write(
        &b_source,
        "import \"../a1/a1.mm\";\natom g(x: i64) -> i64\n  requires: x >= 0;\n  ensures: result >= 0;\n  body: f1(x);\n",
    )
    .expect("write B source");
    assert_success(&fixture.certify(&a0_source, &a0_cert), "certify A0");
    assert_success(&fixture.certify(&a1_source, &a1_cert), "certify A1");
    assert_success(&fixture.certify(&b_source, &b_cert), "certify B");
    let b_before: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&b_cert).unwrap()).expect("parse B certificate");
    let hashes = b_before.atoms[0]
        .dependency_hashes
        .as_ref()
        .expect("generated dependency hashes");
    assert!(hashes.contains_key("atom:f0"), "keys: {hashes:?}");
    assert!(hashes.contains_key("atom:f1"), "keys: {hashes:?}");

    std::fs::write(
        &a0_source,
        "atom f0(x: i64) -> i64\n  requires: x >= 0;\n  ensures: result >= 0;\n  body: x;\n",
    )
    .expect("weaken A0's ensures");
    assert_success(&fixture.certify(&a0_source, &a0_cert), "re-certify A0");
    let stale_f1 = verify_cert(&fixture, &a1_cert, &a1_source, false);
    let stale_f1_text = output_text(&stale_f1);
    assert!(
        stale_f1_text.contains("dependency_changed"),
        "{stale_f1_text}"
    );
    assert!(stale_f1_text.contains("atom:f0"), "{stale_f1_text}");
    assert_success(
        &fixture.certify(&a1_source, &a1_cert),
        "re-certify A1 after A0",
    );
    let stale_g = verify_cert(&fixture, &b_cert, &b_source, false);
    let stale_g_text = output_text(&stale_g);
    assert!(
        stale_g_text.contains("dependency_changed"),
        "{stale_g_text}"
    );
    assert!(stale_g_text.contains("atom:f0"), "{stale_g_text}");

    let typed = Fixture::new();
    let typed_a_dir = typed.root.join("a");
    let typed_b_dir = typed.root.join("b");
    std::fs::create_dir(&typed_a_dir).expect("create typed A directory");
    std::fs::create_dir(&typed_b_dir).expect("create typed B directory");
    let typed_a_source = typed_a_dir.join("a.mm");
    let typed_b_source = typed_b_dir.join("b.mm");
    let typed_a_cert = typed_a_dir.join(".proof-cert.json");
    let typed_b_cert = typed_b_dir.join(".proof-cert.json");
    std::fs::write(
        &typed_a_source,
        "type Positive = i64 where v >= 0;\natom f(x: Positive) -> Positive\n  requires: true;\n  ensures: result >= 0;\n  body: x;\n",
    )
    .expect("write external refined type");
    std::fs::write(
        &typed_b_source,
        "import \"../a/a.mm\";\natom g(x: Positive) -> Positive\n  requires: true;\n  ensures: result >= 0;\n  body: f(x);\n",
    )
    .expect("write typed B source");
    assert_success(
        &typed.certify(&typed_a_source, &typed_a_cert),
        "certify typed A",
    );
    assert_success(
        &typed.certify(&typed_b_source, &typed_b_cert),
        "certify typed B",
    );
    let typed_before: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&typed_b_cert).unwrap())
            .expect("parse typed B certificate");
    assert!(typed_before.atoms[0]
        .dependency_hashes
        .as_ref()
        .unwrap()
        .contains_key("type:Positive"));
    std::fs::write(
        &typed_a_source,
        "type Positive = i64 where v > 0;\natom f(x: Positive) -> Positive\n  requires: true;\n  ensures: result > 0;\n  body: x;\n",
    )
    .expect("change imported refined type");
    assert_success(
        &typed.certify(&typed_a_source, &typed_a_cert),
        "re-certify changed typed A",
    );
    let changed_type = verify_cert(&typed, &typed_b_cert, &typed_b_source, false);
    let changed_type_text = output_text(&changed_type);
    assert!(
        changed_type_text.contains("dependency_changed"),
        "{changed_type_text}"
    );
    assert!(
        changed_type_text.contains("type:Positive"),
        "{changed_type_text}"
    );
}

#[test]
fn trusted_atoms_are_exempt_from_dependency_changes() {
    let fixture = Fixture::new();
    let (a_source, b_source, c_source, a_cert, b_cert) = setup_modules(&fixture);
    std::fs::write(
        &b_source,
        "import \"../a/a.mm\";\ntrusted atom g(x: i64) -> i64\n  requires: x >= 0;\n  ensures: result == x;\n  body: f(x);\n",
    )
    .expect("write trusted B atom");
    assert_success(&fixture.certify(&a_source, &a_cert), "certify A");
    assert_success(&fixture.certify(&b_source, &b_cert), "certify trusted B");
    std::fs::write(
        &a_source,
        "atom f(x: i64) -> i64\n  requires: x >= 0;\n  ensures: result >= 0;\n  body: x;\n",
    )
    .expect("change A's contract");
    assert_success(&fixture.certify(&a_source, &a_cert), "re-certify changed A");

    let checked = verify_cert(&fixture, &b_cert, &b_source, true);
    let checked_text = output_text(&checked);
    assert_success(&checked, "strictly verify trusted B certificate");
    assert!(checked_text.contains("🛡️ g: trusted"), "{checked_text}");
    let imported = fixture.verify(&c_source, &["--strict-imports"]);
    assert_success(&imported, "strictly import trusted B");
}

#[test]
fn strict_manifest_import_checks_path_dependency_freshness() {
    let fixture = Fixture::new();
    let package_root = fixture.root.join("packages");
    let a_dir = package_root.join("a");
    let b_dir = package_root.join("b");
    let a_v2_dir = package_root.join("a-v2");
    for dir in [&a_dir, &b_dir, &a_v2_dir] {
        std::fs::create_dir_all(dir.join("src")).expect("create package source directory");
    }
    std::fs::write(
        a_dir.join("mumei.toml"),
        "[package]\nname = \"fresh_a\"\nversion = \"0.1.0\"\n",
    )
    .expect("write A manifest");
    std::fs::write(
        a_dir.join("src/main.mm"),
        "atom f(x: i64) -> i64\n  requires: x >= 0;\n  ensures: result == x;\n  body: x;\n",
    )
    .expect("write A source");
    let publish_a = fixture.run_in_directory(&a_dir, &["publish"]);
    assert_success(&publish_a, "publish A");
    let published_a = fixture.home.join(".mumei/packages/fresh_a/0.1.0");

    std::fs::write(
        b_dir.join("mumei.toml"),
        format!(
            "[package]\nname = \"fresh_b\"\nversion = \"0.1.0\"\n\n[dependencies]\nfresh_a = {{ path = \"{}\" }}\n",
            published_a.display()
        ),
    )
    .expect("write B manifest");
    std::fs::write(
        b_dir.join("src/main.mm"),
        "atom g(x: i64) -> i64\n  requires: x >= 0;\n  ensures: result == x;\n  body: fresh_a::f(x);\n",
    )
    .expect("write B source");
    let publish_b = fixture.run_in_directory(&b_dir, &["publish"]);
    assert_success(&publish_b, "publish B");
    let published_b = fixture.home.join(".mumei/packages/fresh_b/0.1.0");

    let write_consumer = |directory: &str,
                          b_dep_name: &str,
                          a_dependency: Option<(&str, &Path)>| {
        let consumer = fixture.root.join(directory);
        std::fs::create_dir_all(consumer.join("src")).expect("create consumer directory");
        let mut manifest = format!(
            "[package]\nname = \"fresh_consumer_{directory}\"\nversion = \"0.1.0\"\n\n[dependencies]\n{b_dep_name} = {{ path = \"{}\" }}\n",
            published_b.display(),
        );
        if let Some((a_dep_name, a_path)) = a_dependency {
            manifest.push_str(&format!(
                "{a_dep_name} = {{ path = \"{}\" }}\n",
                a_path.display()
            ));
        }
        std::fs::write(consumer.join("mumei.toml"), manifest).expect("write consumer manifest");
        std::fs::write(
            consumer.join("src/main.mm"),
            format!(
                "atom c(x: i64) -> i64\n  requires: x >= 0;\n  ensures: result == x;\n  body: {b_dep_name}::g(x);\n"
            ),
        )
        .expect("write consumer source");
        consumer
    };
    let consumer_with_a =
        write_consumer("consumer_a_first", "z_dep", Some(("a_dep", &published_a)));
    let with_a = fixture.run_in_directory(
        &consumer_with_a,
        &["verify", "--strict-imports", "src/main.mm"],
    );
    assert_success(&with_a, "strictly verify consumer with A sorting before B");

    let consumer_b_first =
        write_consumer("consumer_b_first", "a_dep", Some(("z_dep", &published_a)));
    let b_first = fixture.run_in_directory(
        &consumer_b_first,
        &["verify", "--strict-imports", "src/main.mm"],
    );
    assert_success(&b_first, "strictly verify consumer with B sorting before A");

    let a_certificate = [
        published_a.join(".proof-cert.json"),
        published_a.join("proof_certificate.json"),
    ]
    .into_iter()
    .find(|candidate| candidate.exists())
    .expect("published A certificate");
    let a_certificate_contents =
        std::fs::read(&a_certificate).expect("read A certificate before removal");
    std::fs::remove_file(&a_certificate).expect("remove A certificate");
    for (directory, b_name, a_name) in [
        ("consumer_missing_a_first", "z_dep", "a_dep"),
        ("consumer_missing_b_first", "a_dep", "z_dep"),
    ] {
        let consumer = write_consumer(directory, b_name, Some((a_name, &published_a)));
        let result = fixture.run_in_directory(&consumer, &["verify", "src/main.mm"]);
        let text = output_text(&result);
        assert_success(
            &result,
            "non-strict manifest import with missing A certificate",
        );
        assert!(
            !text
                .lines()
                .any(|line| { line.contains(format!("Dependency '{b_name}': atom 'g'").as_str()) }),
            "g must remain proven in both dependency-name orders:\n{text}"
        );
    }
    std::fs::write(&a_certificate, a_certificate_contents)
        .expect("restore A certificate for the stale-certificate case");

    std::fs::write(
        published_a.join("src/main.mm"),
        "atom f(x: i64) -> i64\n  requires: x >= 0;\n  ensures: result == x;\n  body: x + 0;\n",
    )
    .expect("change A body without refreshing its certificate");
    for (directory, b_name, a_name) in [
        ("consumer_stale_a_first", "z_dep", "a_dep"),
        ("consumer_stale_b_first", "a_dep", "z_dep"),
    ] {
        let consumer = write_consumer(directory, b_name, Some((a_name, &published_a)));
        let result = fixture.run_in_directory(&consumer, &["verify", "src/main.mm"]);
        let text = output_text(&result);
        assert_success(
            &result,
            "non-strict manifest import with stale A certificate",
        );
        assert!(
            text.lines().any(|line| {
                line.contains(format!("Dependency '{a_name}': atom 'f'").as_str())
                    && line.contains("failed certificate verification")
            }),
            "{text}"
        );
        assert!(
            !text
                .lines()
                .any(|line| { line.contains(format!("Dependency '{b_name}': atom 'g'").as_str()) }),
            "g must remain proven in both dependency-name orders:\n{text}"
        );
    }

    let consumer_without_a = write_consumer("consumer_without_a", "z_dep", None);
    let without_a = fixture.run_in_directory(
        &consumer_without_a,
        &["verify", "--strict-imports", "src/main.mm"],
    );
    let without_a_text = output_text(&without_a);
    assert!(!without_a.status.success(), "{without_a_text}");
    assert!(
        without_a_text.contains("dependency_unchecked"),
        "{without_a_text}"
    );

    std::fs::write(
        a_v2_dir.join("mumei.toml"),
        "[package]\nname = \"fresh_a\"\nversion = \"0.1.1\"\n",
    )
    .expect("write changed A manifest");
    std::fs::write(
        a_v2_dir.join("src/main.mm"),
        "atom f(x: i64) -> i64\n  requires: x >= 0;\n  ensures: result >= 0;\n  body: x;\n",
    )
    .expect("write changed A source");
    let publish_a_v2 = fixture.run_in_directory(&a_v2_dir, &["publish"]);
    assert_success(&publish_a_v2, "publish changed A");
    let published_a_v2 = fixture.home.join(".mumei/packages/fresh_a/0.1.1");
    let changed_consumer = write_consumer(
        "consumer_with_changed_a",
        "a_dep",
        Some(("z_dep", &published_a_v2)),
    );
    let changed = fixture.run_in_directory(
        &changed_consumer,
        &["verify", "--strict-imports", "src/main.mm"],
    );
    let changed_text = output_text(&changed);
    assert!(!changed.status.success(), "{changed_text}");
    assert!(
        changed_text.contains("dependency_changed"),
        "{changed_text}"
    );
    assert!(changed_text.contains("g"), "{changed_text}");
    assert!(changed_text.contains("atom:f"), "{changed_text}");
}

#[test]
fn package_name_alias_does_not_override_declared_dependency_name() {
    let fixture = Fixture::new();
    let package_root = fixture.root.join("packages");
    let alpha_dir = package_root.join("alpha");
    let selected_dir = package_root.join("selected");
    for dir in [&alpha_dir, &selected_dir] {
        std::fs::create_dir_all(dir.join("src")).expect("create package source directory");
    }

    std::fs::write(
        alpha_dir.join("mumei.toml"),
        "[package]\nname = \"fresh_a\"\nversion = \"0.1.0\"\n",
    )
    .expect("write alpha manifest");
    std::fs::write(
        alpha_dir.join("src/main.mm"),
        "atom f(x: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: result == x;\n\
         body: x;\n",
    )
    .expect("write alpha source");

    std::fs::write(
        selected_dir.join("mumei.toml"),
        "[package]\nname = \"selected_package\"\nversion = \"0.1.0\"\n",
    )
    .expect("write selected dependency manifest");
    std::fs::write(
        selected_dir.join("src/main.mm"),
        "atom f(x: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: result == x + 1;\n\
         body: x + 1;\n",
    )
    .expect("write selected dependency source");

    let consumer = fixture.root.join("consumer");
    std::fs::create_dir_all(consumer.join("src")).expect("create consumer directory");
    std::fs::write(
        consumer.join("mumei.toml"),
        format!(
            "[package]\nname = \"alias_collision_consumer\"\nversion = \"0.1.0\"\n\n\
             [dependencies]\nalpha = {{ path = \"{}\" }}\nfresh_a = {{ path = \"{}\" }}\n",
            alpha_dir.display(),
            selected_dir.display(),
        ),
    )
    .expect("write consumer manifest");
    std::fs::write(
        consumer.join("src/main.mm"),
        "atom g(x: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: result == x + 1;\n\
         body: fresh_a::f(x);\n",
    )
    .expect("write consumer source");

    let result = fixture.run_in_directory(&consumer, &["verify", "src/main.mm"]);
    let text = output_text(&result);
    assert_success(&result, "verify package-name alias collision");
    assert!(
        text.lines().any(|line| line.contains("'g': verified")),
        "{text}"
    );
}

#[test]
fn duplicate_package_names_do_not_register_an_ambiguous_alias() {
    let fixture = Fixture::new();
    let package_root = fixture.root.join("packages");
    let x_dir = package_root.join("x");
    let y_dir = package_root.join("y");
    for dir in [&x_dir, &y_dir] {
        std::fs::create_dir_all(dir.join("src")).expect("create package source directory");
        std::fs::write(
            dir.join("mumei.toml"),
            "[package]\nname = \"common\"\nversion = \"0.1.0\"\n",
        )
        .expect("write dependency manifest");
    }
    std::fs::write(
        x_dir.join("src/main.mm"),
        "atom f(x: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: result == x;\n\
         body: x;\n",
    )
    .expect("write x dependency source");
    std::fs::write(
        y_dir.join("src/main.mm"),
        "atom f(x: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: result == x + 1;\n\
         body: x + 1;\n",
    )
    .expect("write y dependency source");

    let consumer = fixture.root.join("consumer");
    std::fs::create_dir_all(consumer.join("src")).expect("create consumer directory");
    std::fs::write(
        consumer.join("mumei.toml"),
        format!(
            "[package]\nname = \"duplicate_package_names_consumer\"\nversion = \"0.1.0\"\n\n\
             [dependencies]\nx = {{ path = \"{}\" }}\ny = {{ path = \"{}\" }}\n",
            x_dir.display(),
            y_dir.display(),
        ),
    )
    .expect("write consumer manifest");
    let source = consumer.join("src/main.mm");
    std::fs::write(
        &source,
        "atom g(x: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: result == x;\n\
         body: x::f(x);\n",
    )
    .expect("write consumer source");

    for _ in 0..3 {
        let result = fixture.run_in_directory(&consumer, &["verify", "src/main.mm"]);
        assert_success(
            &result,
            "verify a qualified dependency with duplicate package names",
        );
    }

    std::fs::write(
        &source,
        "atom g(x: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: result >= 0;\n\
         body: common::f(x);\n",
    )
    .expect("write consumer source using ambiguous package name");
    let missing_alias = fixture.run_in_directory(&consumer, &["verify", "src/main.mm"]);
    let text = output_text(&missing_alias);
    assert!(!missing_alias.status.success(), "{text}");
    assert!(text.contains("Unknown function: common::f"), "{text}");
}

#[test]
fn standard_library_bundle_imports_are_dependency_fresh() {
    let fixture = Fixture::new();
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let std_path = repo_root.join("std");
    let certs_dir = fixture.root.join("std/certs");
    let abs_cert = certs_dir.join("math/abs.proof.json");
    let clamp_cert = certs_dir.join("math/clamp.proof.json");
    std::fs::create_dir_all(abs_cert.parent().unwrap()).expect("create std certificate directory");
    assert_success(
        &fixture.certify(&std_path.join("math/abs.mm"), &abs_cert),
        "certify std/math/abs",
    );
    assert_success(
        &fixture.certify(&std_path.join("math/clamp.mm"), &clamp_cert),
        "certify std/math/clamp",
    );
    let bundle = fixture.root.join("std-proof-bundle.json");
    let script = repo_root
        .join("scripts/bundle_std_certs.py")
        .display()
        .to_string();
    let bundled = Command::new("python3")
        .args([
            script,
            "--certs-dir".to_string(),
            certs_dir.display().to_string(),
            "--output".to_string(),
            bundle.display().to_string(),
            "--mumei-version".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        ])
        .current_dir(&fixture.root)
        .output()
        .expect("bundle std certificates");
    assert!(
        bundled.status.success(),
        "bundle failed\n{}",
        output_text(&bundled)
    );
    let source = fixture.root.join("std-consumer.mm");
    std::fs::write(
        &source,
        "import \"std/math/abs\" as abs;\nimport \"std/math/clamp\" as clamp;\natom use_abs(x: i64) -> i64\n  requires: true;\n  ensures: result >= 0;\n  body: abs::abs_saturating(x);\natom use_clamp(x: i64) -> i64\n  requires: true;\n  ensures: result >= 0;\n  body: clamp::clamp_nonneg(x, 10);\n",
    )
    .expect("write std bundle consumer");
    let imported =
        fixture.verify_with_proof_bundle(&source, &["--strict-imports"], &bundle, &std_path);
    let imported_text = output_text(&imported);
    assert_success(&imported, "strictly import bundled std certificates");
    assert!(
        !imported_text.contains("dependency_changed"),
        "{imported_text}"
    );
    assert!(
        !imported_text.contains("dependency_unchecked"),
        "{imported_text}"
    );
}

#[test]
fn imported_refined_field_body_reference_invalidates_certificate_and_cache() {
    let fixture = Fixture::new();
    let a_source = fixture.root.join("a.mm");
    let b_source = fixture.root.join("b.mm");
    let certificate = fixture.root.join("a.proof.json");
    std::fs::write(
        &b_source,
        "type Pos = i64 where v >= 1;\nstruct P { x: Pos }\n",
    )
    .expect("write refined field module");
    std::fs::write(
        &a_source,
        "import \"./b.mm\";\n\
         atom g(a: i64) -> i64\n\
         requires: a >= 1;\n\
         ensures: result >= 1;\n\
         body: {\n\
           let p = P { x: a };\n\
           p.x\n\
         };\n",
    )
    .expect("write struct-literal consumer");

    assert_success(
        &fixture.certify(&a_source, &certificate),
        "certify struct-literal consumer",
    );
    let unchanged_certificate = verify_cert(&fixture, &certificate, &a_source, false);
    let unchanged_certificate_text = output_text(&unchanged_certificate);
    assert_eq!(
        unchanged_certificate.status.code(),
        Some(0),
        "{unchanged_certificate_text}"
    );
    assert!(
        unchanged_certificate_text.contains("g: proven"),
        "{unchanged_certificate_text}"
    );

    let initial_verify = fixture.verify(&a_source, &[]);
    assert_success(&initial_verify, "verify unchanged consumer");
    let cached_verify = fixture.verify(&a_source, &[]);
    let cached_verify_text = output_text(&cached_verify);
    assert_success(&cached_verify, "verify cached unchanged consumer");
    assert!(
        cached_verify_text.contains("skipped (unchanged)"),
        "{cached_verify_text}"
    );

    std::fs::write(
        &b_source,
        "type Pos = i64 where v >= 2;\nstruct P { x: Pos }\n",
    )
    .expect("strengthen imported refined type");
    let changed_certificate = verify_cert(&fixture, &certificate, &a_source, false);
    let changed_certificate_text = output_text(&changed_certificate);
    assert_eq!(
        changed_certificate.status.code(),
        Some(0),
        "{changed_certificate_text}"
    );
    assert!(
        changed_certificate_text.contains("g: dependency_changed"),
        "{changed_certificate_text}"
    );
    assert!(
        changed_certificate_text.contains("type:Pos"),
        "{changed_certificate_text}"
    );

    let changed_verify = fixture.verify(&a_source, &[]);
    let changed_verify_text = output_text(&changed_verify);
    assert!(
        !changed_verify.status.success(),
        "changed refined type unexpectedly verified\n{changed_verify_text}"
    );
    assert!(
        !changed_verify_text.contains("skipped (unchanged)"),
        "{changed_verify_text}"
    );
}

#[test]
fn imported_trait_method_constraint_change_invalidates_certificate_and_cache() {
    let fixture = Fixture::new();
    let a_source = fixture.root.join("a.mm");
    let b_source = fixture.root.join("b.mm");
    let certificate = fixture.root.join("a.proof.json");
    std::fs::write(
        &b_source,
        "trait Picker {\n\
         fn pick(a: Self where v >= 0) -> Self;\n\
         }\n\
         impl Picker for i64 {\n\
         fn pick(a: i64) -> i64 { a }\n\
         }\n\
         atom pick(a: i64) -> i64\n\
         requires: true;\n\
         ensures: true;\n\
         body: a;\n",
    )
    .expect("write trait method and implementation");
    std::fs::write(
        &a_source,
        "import \"./b.mm\";\n\
         atom g(x: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: true;\n\
         body: pick(x);\n",
    )
    .expect("write trait method caller");

    assert_success(
        &fixture.certify(&a_source, &certificate),
        "certify trait method caller",
    );
    let cert: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&certificate).unwrap()).expect("parse certificate");
    let g = cert
        .atoms
        .iter()
        .find(|atom| atom.name == "g")
        .expect("g certificate");
    let dependency_hashes = g
        .dependency_hashes
        .as_ref()
        .expect("generated dependency hashes");
    assert!(dependency_hashes.contains_key("atom:pick"));

    let unchanged_certificate = verify_cert(&fixture, &certificate, &a_source, false);
    let unchanged_certificate_text = output_text(&unchanged_certificate);
    assert_eq!(
        unchanged_certificate.status.code(),
        Some(0),
        "{unchanged_certificate_text}"
    );
    assert!(
        unchanged_certificate_text.contains("g: proven"),
        "{unchanged_certificate_text}"
    );

    let initial_verify = fixture.verify(&a_source, &[]);
    assert_success(&initial_verify, "verify unchanged trait method caller");
    let cached_verify = fixture.verify(&a_source, &[]);
    let cached_verify_text = output_text(&cached_verify);
    assert_success(&cached_verify, "verify cached trait method caller");
    assert!(
        cached_verify_text.contains("skipped (unchanged)"),
        "{cached_verify_text}"
    );

    std::fs::write(
        &b_source,
        "trait Picker {\n\
         fn pick(a: Self where v >= 5) -> Self;\n\
         }\n\
         impl Picker for i64 {\n\
         fn pick(a: i64) -> i64 { a }\n\
         }\n\
         atom pick(a: i64) -> i64\n\
         requires: true;\n\
         ensures: true;\n\
         body: a;\n",
    )
    .expect("strengthen trait method parameter constraint");

    let changed_certificate = verify_cert(&fixture, &certificate, &a_source, false);
    let changed_certificate_text = output_text(&changed_certificate);
    assert_eq!(
        changed_certificate.status.code(),
        Some(0),
        "{changed_certificate_text}"
    );
    assert!(
        changed_certificate_text.contains("g: dependency_changed"),
        "{changed_certificate_text}"
    );
    assert!(
        changed_certificate_text.contains("atom:pick"),
        "{changed_certificate_text}"
    );

    let changed_verify = fixture.verify(&a_source, &[]);
    let changed_verify_text = output_text(&changed_verify);
    assert!(
        !changed_verify.status.success(),
        "changed trait constraint unexpectedly verified\n{changed_verify_text}"
    );
    assert!(
        !changed_verify_text.contains("skipped (unchanged)"),
        "{changed_verify_text}"
    );
    assert!(
        changed_verify_text
            .contains("Call to 'pick': trait method parameter constraint 'v >= 5' not satisfied"),
        "{changed_verify_text}"
    );
}

#[test]
fn imported_acquired_resource_invariant_change_invalidates_certificate_and_cache() {
    let fixture = Fixture::new();
    let certificate = fixture.root.join("a.proof.json");
    let (a_source, _) = resource_freshness_sources(&fixture, 0, false);

    assert_success(
        &fixture.certify(&a_source, &certificate),
        "certify caller with acquired resource",
    );
    let cert: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&certificate).unwrap()).expect("parse certificate");
    let main = cert
        .atoms
        .iter()
        .find(|atom| atom.name == "main")
        .expect("main certificate");
    assert!(main
        .dependency_hashes
        .as_ref()
        .expect("generated dependency hashes")
        .contains_key("resource:counter"));

    let mut legacy_cert: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&certificate).unwrap())
            .expect("parse certificate for legacy-key check");
    let main = legacy_cert
        .atoms
        .iter_mut()
        .find(|atom| atom.name == "main")
        .expect("main certificate");
    assert!(main
        .dependency_hashes
        .as_mut()
        .expect("generated dependency hashes")
        .remove("resource:counter")
        .is_some());
    refresh_certificate_integrity(&mut legacy_cert);
    let legacy_certificate = fixture.root.join("a.legacy.proof.json");
    std::fs::write(
        &legacy_certificate,
        serde_json::to_vec_pretty(&legacy_cert).unwrap(),
    )
    .expect("write certificate without resource dependency key");
    let unchecked = verify_cert(&fixture, &legacy_certificate, &a_source, false);
    let unchecked_text = output_text(&unchecked);
    assert_eq!(unchecked.status.code(), Some(0), "{unchecked_text}");
    assert!(
        unchecked_text.contains("main: dependency_unchecked"),
        "{unchecked_text}"
    );
    assert!(
        unchecked_text.contains("resource:counter"),
        "{unchecked_text}"
    );

    let initial_verify = fixture.verify(&a_source, &[]);
    assert_success(
        &initial_verify,
        "verify caller with unchanged acquired resource",
    );
    let cached_verify = fixture.verify(&a_source, &[]);
    let cached_verify_text = output_text(&cached_verify);
    assert_success(&cached_verify, "verify cached acquired resource caller");
    assert!(
        cached_verify_text.contains("skipped (unchanged)"),
        "{cached_verify_text}"
    );

    resource_freshness_sources(&fixture, 5, false);
    let changed_certificate = verify_cert(&fixture, &certificate, &a_source, false);
    let changed_certificate_text = output_text(&changed_certificate);
    assert_eq!(
        changed_certificate.status.code(),
        Some(0),
        "{changed_certificate_text}"
    );
    assert!(
        changed_certificate_text.contains("main: dependency_changed"),
        "{changed_certificate_text}"
    );
    assert!(
        changed_certificate_text.contains("resource:counter"),
        "{changed_certificate_text}"
    );

    let changed_verify = fixture.verify(&a_source, &[]);
    let changed_verify_text = output_text(&changed_verify);
    assert!(
        !changed_verify.status.success(),
        "changed acquired resource invariant unexpectedly verified\n{changed_verify_text}"
    );
    assert!(
        !changed_verify_text.contains("skipped (unchanged)"),
        "{changed_verify_text}"
    );
    assert!(
        changed_verify_text
            .contains("resource 'counter' invariant does not hold for the initial state"),
        "{changed_verify_text}"
    );
}

#[test]
fn imported_declared_resource_invariant_change_is_a_dependency_change() {
    let fixture = Fixture::new();
    let certificate = fixture.root.join("a.proof.json");
    let (a_source, _) = resource_freshness_sources(&fixture, 0, true);

    assert_success(
        &fixture.certify(&a_source, &certificate),
        "certify caller with declared resource",
    );
    let cert: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&certificate).unwrap()).expect("parse certificate");
    let main = cert
        .atoms
        .iter()
        .find(|atom| atom.name == "main")
        .expect("main certificate");
    assert!(main
        .dependency_hashes
        .as_ref()
        .expect("generated dependency hashes")
        .contains_key("resource:counter"));

    resource_freshness_sources(&fixture, 5, true);
    let changed_certificate = verify_cert(&fixture, &certificate, &a_source, false);
    let changed_certificate_text = output_text(&changed_certificate);
    assert_eq!(
        changed_certificate.status.code(),
        Some(0),
        "{changed_certificate_text}"
    );
    assert!(
        changed_certificate_text.contains("main: dependency_changed"),
        "{changed_certificate_text}"
    );
    assert!(
        changed_certificate_text.contains("resource:counter"),
        "{changed_certificate_text}"
    );
}

#[test]
fn imported_unused_module_resource_invalidates_cached_verify() {
    let fixture = Fixture::new();
    let certificate = fixture.root.join("a.proof.json");
    let (a_source, _) = module_resource_freshness_sources(&fixture, 0);

    assert_success(
        &fixture.certify(&a_source, &certificate),
        "certify unrelated atom with imported module resource",
    );

    let initial_verify = fixture.verify(&a_source, &[]);
    assert_success(
        &initial_verify,
        "verify unrelated atom with unchanged module resource",
    );
    let cached_verify = fixture.verify(&a_source, &[]);
    let cached_verify_text = output_text(&cached_verify);
    assert_success(&cached_verify, "verify cached unrelated atom");
    assert!(
        cached_verify_text.contains("skipped (unchanged)"),
        "{cached_verify_text}"
    );

    module_resource_freshness_sources(&fixture, 5);
    let changed_certificate = verify_cert(&fixture, &certificate, &a_source, false);
    let changed_certificate_text = output_text(&changed_certificate);
    assert_eq!(
        changed_certificate.status.code(),
        Some(0),
        "{changed_certificate_text}"
    );
    assert!(
        changed_certificate_text.contains("g: dependency_changed"),
        "{changed_certificate_text}"
    );
    assert!(
        changed_certificate_text.contains("resource:counter"),
        "{changed_certificate_text}"
    );

    let changed_verify = fixture.verify(&a_source, &[]);
    let changed_verify_text = output_text(&changed_verify);
    assert!(
        !changed_verify.status.success(),
        "changed module resource invariant unexpectedly verified\n{changed_verify_text}"
    );
    assert!(
        !changed_verify_text.contains("skipped (unchanged)"),
        "{changed_verify_text}"
    );
    assert!(
        changed_verify_text
            .contains("resource 'counter' invariant does not hold for the initial state"),
        "{changed_verify_text}"
    );
}

#[test]
fn strict_import_keeps_resource_free_module_certificate_proven() {
    let fixture = Fixture::new();
    let b_dir = fixture.root.join("b");
    let c_dir = fixture.root.join("c");
    let a_dir = fixture.root.join("a");
    for dir in [&a_dir, &b_dir, &c_dir] {
        std::fs::create_dir(dir).expect("create module directory");
    }
    let b_source = b_dir.join("b.mm");
    let c_source = c_dir.join("c.mm");
    let a_source = a_dir.join("a.mm");
    let b_certificate = b_dir.join(".proof-cert.json");
    let c_certificate = c_dir.join(".proof-cert.json");

    std::fs::write(
        &b_source,
        "resource counter { value: i64 } priority: 1 mode: exclusive invariant: counter.value >= 0;\n\
         atom b_fn(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n",
    )
    .expect("write resource-bearing B");
    std::fs::write(
        &c_source,
        "atom c_fn(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n",
    )
    .expect("write resource-free C");
    assert_success(
        &fixture.certify(&b_source, &b_certificate),
        "certify resource-bearing B standalone",
    );
    assert_success(
        &fixture.certify(&c_source, &c_certificate),
        "certify resource-free C standalone",
    );
    let c_certificate_check = verify_cert(&fixture, &c_certificate, &c_source, false);
    let c_certificate_text = output_text(&c_certificate_check);
    assert_success(&c_certificate_check, "check standalone C certificate");
    assert!(
        c_certificate_text.contains("c_fn: proven"),
        "{c_certificate_text}"
    );

    std::fs::write(
        &a_source,
        "import \"../b/b.mm\";\n\
         import \"../c/c.mm\";\n\
         atom main(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: c_fn(x);\n",
    )
    .expect("write importer");
    let imported = fixture.verify(&a_source, &["--strict-imports"]);
    let imported_text = output_text(&imported);
    assert_success(&imported, "strictly import C after resource-bearing B");
    assert!(
        !imported_text.contains("dependency_unchecked"),
        "{imported_text}"
    );
}

#[test]
fn strict_alias_import_does_not_key_alias_resource_name() {
    let fixture = Fixture::new();
    let b_dir = fixture.root.join("b");
    let a_dir = fixture.root.join("a");
    for dir in [&a_dir, &b_dir] {
        std::fs::create_dir(dir).expect("create module directory");
    }
    let b_source = b_dir.join("b.mm");
    let a_source = a_dir.join("a.mm");
    let b_certificate = b_dir.join(".proof-cert.json");

    std::fs::write(
        &b_source,
        "resource counter { value: i64 } priority: 1 mode: exclusive invariant: true;\n\
         atom b_fn(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n",
    )
    .expect("write resource-bearing B");
    assert_success(
        &fixture.certify(&b_source, &b_certificate),
        "certify resource-bearing B standalone",
    );
    let standalone = verify_cert(&fixture, &b_certificate, &b_source, false);
    let standalone_text = output_text(&standalone);
    assert_success(&standalone, "check standalone B certificate");
    assert!(
        standalone_text.contains("b_fn: proven"),
        "{standalone_text}"
    );

    std::fs::write(
        &a_source,
        "import \"../b/b.mm\" as b;\n\
         atom main(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n",
    )
    .expect("write alias importer");
    let unchanged = fixture.verify(&a_source, &["--strict-imports"]);
    let unchanged_text = output_text(&unchanged);
    assert_success(&unchanged, "strictly import unchanged B through an alias");
    assert!(
        !unchanged_text.contains("dependency_unchecked"),
        "{unchanged_text}"
    );

    std::fs::write(
        &b_source,
        "resource counter { value: i64 } priority: 1 mode: exclusive invariant: 1 == 1;\n\
         atom b_fn(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n",
    )
    .expect("change B resource invariant");
    let changed = fixture.verify(&a_source, &["--strict-imports"]);
    let changed_text = output_text(&changed);
    assert!(!changed.status.success(), "{changed_text}");
    assert!(
        changed_text.contains("dependency_changed"),
        "{changed_text}"
    );
    assert!(changed_text.contains("resource:counter"), "{changed_text}");
}

#[test]
fn strict_import_tracks_resources_from_nested_import_closure() {
    let fixture = Fixture::new();
    let b_dir = fixture.root.join("b");
    let c_dir = fixture.root.join("c");
    let a_dir = fixture.root.join("a");
    for dir in [&a_dir, &b_dir, &c_dir] {
        std::fs::create_dir(dir).expect("create module directory");
    }
    let b_source = b_dir.join("b.mm");
    let c_source = c_dir.join("c.mm");
    let a_source = a_dir.join("a.mm");
    let b_certificate = b_dir.join(".proof-cert.json");
    let c_certificate = c_dir.join(".proof-cert.json");

    std::fs::write(
        &b_source,
        "resource counter { value: i64 } priority: 1 mode: exclusive invariant: counter.value >= 0;\n\
         atom b_fn(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n",
    )
    .expect("write B with resource");
    std::fs::write(
        &c_source,
        "import \"../b/b.mm\";\n\
         atom c_fn(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n",
    )
    .expect("write C importing B");
    assert_success(
        &fixture.certify(&b_source, &b_certificate),
        "certify trusted B",
    );
    assert_success(
        &fixture.certify(&c_source, &c_certificate),
        "certify C standalone with B imported",
    );
    let standalone_c = verify_cert(&fixture, &c_certificate, &c_source, false);
    let standalone_c_text = output_text(&standalone_c);
    assert_success(&standalone_c, "check standalone C certificate");
    assert!(
        standalone_c_text.contains("c_fn: proven"),
        "{standalone_c_text}"
    );

    std::fs::write(
        &a_source,
        "import \"../b/b.mm\";\n\
         import \"../c/c.mm\";\n\
         atom main(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: c_fn(x);\n",
    )
    .expect("write importer of C");
    let unchanged = fixture.verify(&a_source, &["--strict-imports"]);
    assert_success(&unchanged, "strictly import unchanged C and B");

    std::fs::write(
        &b_source,
        "resource counter { value: i64 } priority: 1 mode: exclusive invariant: counter.value >= 5;\n\
         atom b_fn(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n",
    )
    .expect("change B resource invariant");
    let changed = fixture.verify(&a_source, &["--strict-imports"]);
    let changed_text = output_text(&changed);
    assert!(
        !changed.status.success(),
        "strict import unexpectedly accepted stale C certificate\n{changed_text}"
    );
    assert!(
        changed_text.contains("dependency_changed"),
        "{changed_text}"
    );
    assert!(changed_text.contains("resource:counter"), "{changed_text}");
}

#[test]
fn strict_import_tracks_unrefined_type_definitions() {
    // An unreferenced refined type still gates every atom's verification: the
    // spec sanity phase SAT-checks every type in the module environment, so a
    // predicate change can flip b_fn's verdict and must surface as a dependency
    // change even though nothing references the type.
    let fixture = Fixture::new();
    let b_dir = fixture.root.join("b");
    let a_dir = fixture.root.join("a");
    for dir in [&a_dir, &b_dir] {
        std::fs::create_dir(dir).expect("create module directory");
    }
    let b_source = b_dir.join("b.mm");
    let a_source = a_dir.join("a.mm");
    let b_certificate = b_dir.join(".proof-cert.json");

    let b_source_text = |predicate: &str| {
        format!(
            "type Positive = i64 where v >= 0;\n\
             type Limit = i64 where {predicate};\n\
             atom b_fn(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: x;\n"
        )
    };
    std::fs::write(&b_source, b_source_text("v < 100")).expect("write B with refined types");
    assert_success(
        &fixture.certify(&b_source, &b_certificate),
        "certify B with an unreferenced refined type",
    );
    let certificate: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&b_certificate).unwrap())
            .expect("parse B certificate");
    let b_fn = certificate
        .atoms
        .iter()
        .find(|atom| atom.name == "b_fn")
        .expect("b_fn certificate");
    let dependency_hashes = b_fn
        .dependency_hashes
        .as_ref()
        .expect("generated dependency hashes");
    assert!(dependency_hashes.contains_key("type:Limit"));
    assert!(dependency_hashes.contains_key("type:Positive"));

    std::fs::write(
        &a_source,
        "import \"../b/b.mm\";\n\
         atom main(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n",
    )
    .expect("write importer");
    let unchanged = fixture.verify(&a_source, &["--strict-imports"]);
    let unchanged_text = output_text(&unchanged);
    assert_success(
        &unchanged,
        "strictly import B holding an unreferenced refined type",
    );
    assert!(
        !unchanged_text.contains("dependency_unchecked"),
        "{unchanged_text}"
    );

    // Change the unreferenced predicate: the certificate must report the type
    // as a changed dependency instead of staying stale-proven.
    std::fs::write(&b_source, b_source_text("v <= 50")).expect("change the unreferenced predicate");
    let changed = verify_cert(&fixture, &b_certificate, &b_source, false);
    let changed_text = output_text(&changed);
    assert_eq!(changed.status.code(), Some(0), "{changed_text}");
    assert!(
        changed_text.contains("dependency_changed"),
        "{changed_text}"
    );
    assert!(changed_text.contains("type:Limit"), "{changed_text}");

    // Make it contradictory: every atom's verification now fails the all-types
    // SAT check, including the importer's.
    std::fs::write(&b_source, b_source_text("v < 0 && v > 0"))
        .expect("make the unreferenced predicate contradictory");
    let unsat = fixture.verify(&a_source, &["--strict-imports"]);
    let unsat_text = output_text(&unsat);
    assert!(
        !unsat.status.success(),
        "importing B with an unsatisfiable refined type must fail\n{unsat_text}"
    );
}

#[test]
fn strict_import_tracks_unrefined_struct_and_enum_definitions() {
    // An unreferenced struct still gates every atom's verification: the
    // projection-collision rejection scans every struct in the module
    // environment, and a nested enum participates in the same flattened paths.
    let fixture = Fixture::new();
    let b_dir = fixture.root.join("b");
    let a_dir = fixture.root.join("a");
    for dir in [&a_dir, &b_dir] {
        std::fs::create_dir(dir).expect("create module directory");
    }
    let b_source = b_dir.join("b.mm");
    let a_source = a_dir.join("a.mm");
    let b_certificate = b_dir.join(".proof-cert.json");

    let b_source_text = |struct_field: &str, enum_payload: &str| {
        format!(
            "struct Inner {{ x: i64 }}\n\
             enum Step {{ A({enum_payload}: i64), B }}\n\
             struct Pair {{ inner: Inner, step: Step, other: {struct_field} }}\n\
             atom b_fn(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: x;\n"
        )
    };
    std::fs::write(&b_source, b_source_text("i64", "v")).expect("write B with struct and enum");
    assert_success(
        &fixture.certify(&b_source, &b_certificate),
        "certify B with unreferenced struct and enum",
    );
    let certificate: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&b_certificate).unwrap())
            .expect("parse B certificate");
    let b_fn = certificate
        .atoms
        .iter()
        .find(|atom| atom.name == "b_fn")
        .expect("b_fn certificate");
    let dependency_hashes = b_fn
        .dependency_hashes
        .as_ref()
        .expect("generated dependency hashes");
    assert!(dependency_hashes.contains_key("struct:Pair"));
    assert!(dependency_hashes.contains_key("struct:Inner"));
    assert!(dependency_hashes.contains_key("enum:Step"));

    std::fs::write(
        &a_source,
        "import \"../b/b.mm\";\n\
         atom main(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n",
    )
    .expect("write importer");
    let unchanged = fixture.verify(&a_source, &["--strict-imports"]);
    let unchanged_text = output_text(&unchanged);
    assert_success(
        &unchanged,
        "strictly import B holding unreferenced struct and enum definitions",
    );
    assert!(
        !unchanged_text.contains("dependency_unchecked"),
        "{unchanged_text}"
    );

    // Rename the field so 'inner_x' collides with Inner's flattened 'inner_x'
    // projection — the module-level gate now rejects every atom in B.
    std::fs::write(
        &b_source,
        b_source_text("i64", "v").replace("other: i64", "inner_x: i64"),
    )
    .expect("rename the struct field into a projection collision");
    let changed = fixture.verify(&a_source, &["--strict-imports"]);
    let changed_text = output_text(&changed);
    assert!(
        !changed.status.success(),
        "importing B with a colliding projection must fail\n{changed_text}"
    );
    assert!(
        changed_text.contains("projection paths collide")
            || changed_text.contains("dependency_changed"),
        "{changed_text}"
    );
}

#[test]
fn manifest_dependency_definitions_stay_in_root_dependency_keys() {
    // Manifest-only dependencies load into the root file's module environment
    // just like `import` statements do, so an unused definition there still
    // gates the consumer's verification. Recording the root import closure
    // must not drop its key — otherwise the certificate stays stale-proven.
    let fixture = Fixture::new();
    let dep_dir = fixture.root.join("dep");
    let consumer_dir = fixture.root.join("consumer");
    std::fs::create_dir_all(dep_dir.join("src")).expect("create dep source directory");
    std::fs::create_dir_all(consumer_dir.join("src")).expect("create consumer source directory");

    std::fs::write(
        dep_dir.join("mumei.toml"),
        "[package]\nname = \"dep_pkg\"\nversion = \"0.1.0\"\n",
    )
    .expect("write dep manifest");
    let dep_main = dep_dir.join("src/main.mm");
    let dep_source = |predicate: &str| {
        format!(
            "type Limit = i64 where {predicate};\n\
             atom dep_fn(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: x;\n"
        )
    };
    std::fs::write(&dep_main, dep_source("v >= 0")).expect("write dep source");

    std::fs::write(
        consumer_dir.join("mumei.toml"),
        format!(
            "[package]\nname = \"consumer_pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\ndep_pkg = {{ path = \"{}\" }}\n",
            dep_dir.display()
        ),
    )
    .expect("write consumer manifest");
    std::fs::write(
        consumer_dir.join("src/main.mm"),
        "atom main(x: i64) -> i64\n  requires: true;\n  ensures: result == x;\n  body: x;\n",
    )
    .expect("write consumer source");

    let certificate_path = consumer_dir.join(".proof-cert.json");
    assert_success(
        &fixture.run_in_directory(
            &consumer_dir,
            &[
                "verify",
                "--proof-cert",
                "--output",
                certificate_path
                    .to_str()
                    .expect("certificate path is UTF-8"),
                "src/main.mm",
            ],
        ),
        "certify the consumer against a manifest dependency",
    );
    let certificate: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&certificate_path).unwrap())
            .expect("parse consumer certificate");
    let main_atom = certificate
        .atoms
        .iter()
        .find(|atom| atom.name == "main")
        .expect("main certificate");
    assert!(
        main_atom
            .dependency_hashes
            .as_ref()
            .expect("generated dependency hashes")
            .contains_key("type:Limit"),
        "the manifest dependency's unused refined type must be keyed"
    );

    std::fs::write(&dep_main, dep_source("v <= 50")).expect("change the dep predicate");
    let changed = fixture.run_in_directory(
        &consumer_dir,
        &[
            "verify-cert",
            certificate_path
                .to_str()
                .expect("certificate path is UTF-8"),
            "src/main.mm",
        ],
    );
    let changed_text = output_text(&changed);
    assert_eq!(changed.status.code(), Some(0), "{changed_text}");
    assert!(
        changed_text.contains("dependency_changed"),
        "{changed_text}"
    );
    assert!(changed_text.contains("type:Limit"), "{changed_text}");
}

#[test]
fn manifest_dependency_of_dependency_stays_in_dep_dependency_keys() {
    // B certified standalone keys the definitions its own manifest dependency
    // A provides. When C loads both packages, B's certificate must still match:
    // B's entry closure inside C's environment must contain A's entry, or the
    // stored keys recompute as missing and strict imports reject a fresh
    // certificate as dependency_unchecked.
    let fixture = Fixture::new();
    let a_dir = fixture.root.join("dep_a");
    let b_dir = fixture.root.join("dep_b");
    let c_dir = fixture.root.join("consumer");
    for dir in [&a_dir, &b_dir, &c_dir] {
        std::fs::create_dir_all(dir.join("src")).expect("create package source directory");
    }

    std::fs::write(
        a_dir.join("mumei.toml"),
        "[package]\nname = \"dep_a\"\nversion = \"0.1.0\"\n",
    )
    .expect("write A manifest");
    let a_main = a_dir.join("src/main.mm");
    let a_source = |predicate: &str| {
        format!(
            "type Limit = i64 where {predicate};\n\
             atom a_fn(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: x;\n"
        )
    };
    std::fs::write(&a_main, a_source("v >= 0")).expect("write A source");

    std::fs::write(
        b_dir.join("mumei.toml"),
        format!(
            "[package]\nname = \"dep_b\"\nversion = \"0.1.0\"\n\n[dependencies]\ndep_a = {{ path = \"{}\" }}\n",
            a_dir.display()
        ),
    )
    .expect("write B manifest");
    std::fs::write(
        b_dir.join("src/main.mm"),
        "atom b_fn(x: i64) -> i64\n  requires: true;\n  ensures: result == x;\n  body: x;\n",
    )
    .expect("write B source");
    let b_certificate = b_dir.join(".proof-cert.json");
    assert_success(
        &fixture.run_in_directory(
            &b_dir,
            &[
                "verify",
                "--proof-cert",
                "--output",
                b_certificate.to_str().expect("certificate path is UTF-8"),
                "src/main.mm",
            ],
        ),
        "certify B against its manifest dependency",
    );
    let certificate: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&b_certificate).unwrap())
            .expect("parse B certificate");
    let b_fn = certificate
        .atoms
        .iter()
        .find(|atom| atom.name == "b_fn")
        .expect("b_fn certificate");
    assert!(
        b_fn.dependency_hashes
            .as_ref()
            .expect("generated dependency hashes")
            .contains_key("type:Limit"),
        "B's manifest-only dependency must contribute its refined type key"
    );

    // A needs its own certificate for strict import too.
    assert_success(
        &fixture.run_in_directory(
            &a_dir,
            &[
                "verify",
                "--proof-cert",
                "--output",
                a_dir
                    .join(".proof-cert.json")
                    .to_str()
                    .expect("certificate path is UTF-8"),
                "src/main.mm",
            ],
        ),
        "certify A",
    );

    // C declares both packages; strict import must accept B's certificate
    // unchanged — B's closure has to reach A's entry for `type:Limit`.
    std::fs::write(
        c_dir.join("mumei.toml"),
        format!(
            "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\n\n[dependencies]\nx_a = {{ path = \"{}\" }}\ny_b = {{ path = \"{}\" }}\n",
            a_dir.display(),
            b_dir.display()
        ),
    )
    .expect("write C manifest");
    std::fs::write(
        c_dir.join("src/main.mm"),
        "atom main(x: i64) -> i64\n  requires: true;\n  ensures: result == x;\n  body: dep_b::b_fn(x);\n",
    )
    .expect("write C source");
    let unchanged =
        fixture.run_in_directory(&c_dir, &["verify", "--strict-imports", "src/main.mm"]);
    let unchanged_text = output_text(&unchanged);
    assert_success(
        &unchanged,
        "strictly import B whose certificate keys a dep-of-dep definition",
    );
    assert!(
        !unchanged_text.contains("dependency_unchecked"),
        "{unchanged_text}"
    );

    // Changing A's unused refined type must invalidate B's certificate in C's
    // environment the same way it does for B standalone. A's own certificate
    // also goes stale, so re-certify A first: the assertion below then
    // isolates B's invalidation instead of passing only because A's check
    // (sorted first) already failed.
    std::fs::write(&a_main, a_source("v <= 50")).expect("change A's predicate");
    let recertify_a = fixture.run_in_directory(
        &a_dir,
        &[
            "verify",
            "--proof-cert",
            "--output",
            a_dir
                .join(".proof-cert.json")
                .to_str()
                .expect("certificate path is UTF-8"),
            "src/main.mm",
        ],
    );
    assert_success(&recertify_a, "re-certify A after changing its predicate");
    let changed = fixture.run_in_directory(&c_dir, &["verify", "--strict-imports", "src/main.mm"]);
    let changed_text = output_text(&changed);
    assert!(
        !changed.status.success() && changed_text.contains("dependency_changed"),
        "B's stale dep-of-dep definition must surface under strict import\n{changed_text}"
    );
}
