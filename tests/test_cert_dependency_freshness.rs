use mumei_core::proof_cert::{refresh_certificate_integrity, LeanResultMetadata, ProofCertificate};
use mumei_core::verification;
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
    assert_eq!(changed.status.code(), Some(1), "{}", output_text(&changed));
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
    assert!(!changed_type.status.success(), "{type_text}");
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
