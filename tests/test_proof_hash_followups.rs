use mumei_core::proof_cert::{refresh_certificate_integrity, ProofCertificate};
use serde_json::Value;
use std::collections::BTreeMap;
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
            "mumei_proof_hash_followups_{}_{}_{}",
            std::process::id(),
            nonce,
            DIR_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir(&root).expect("create exclusive test directory");
        let home = root.join("home");
        let reports = root.join("reports");
        std::fs::create_dir(&home).expect("create isolated home");
        std::fs::create_dir(&reports).expect("create report directory");
        Self {
            root,
            home,
            reports,
        }
    }

    fn write(&self, relative: &str, source: &str) -> PathBuf {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture path has parent"))
            .expect("create fixture parent");
        std::fs::write(&path, source).expect("write fixture source");
        path
    }

    fn run(&self, args: &[String]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args(args)
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env_remove("MUMEI_PROOF_BUNDLE")
            .env_remove("CARGO_MANIFEST_DIR")
            .output()
            .expect("run mumei")
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

    fn verify_cert(&self, certificate: &Path, source: &Path, strict: bool) -> Output {
        let mut args = vec![
            "verify-cert".to_string(),
            certificate.display().to_string(),
            source.display().to_string(),
        ];
        if strict {
            args.push("--strict".to_string());
        }
        self.run(&args)
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

fn write_effect_project(fixture: &Fixture, pre: &str, post: &str) -> (PathBuf, PathBuf) {
    let a = fixture.write(
        "a/a.mm",
        &format!(
            "effect File\n\
             states: [Open, Closed];\n\
             initial: Open;\n\
             transition close: Open -> Closed;\n\
             transition open: Closed -> Open;\n\
             trusted atom f(x: i64) -> i64\n\
             effects: [File];\n\
             effect_pre: {{ File: {pre} }};\n\
             effect_post: {{ File: {post} }};\n\
             requires: x >= 0;\n\
             ensures: result == x;\n\
             body: x;\n"
        ),
    );
    let b = fixture.write(
        "b.mm",
        "import \"a/a.mm\";\n\
         atom g(x: i64) -> i64\n\
         effects: [File];\n\
         effect_pre: { File: Open };\n\
         effect_post: { File: Open };\n\
         requires: x >= 0;\n\
         ensures: result == x;\n\
         body: f(x);\n",
    );
    (a, b)
}

fn collision_project(
    fixture: &Fixture,
    a_ensures: &str,
    b_ensures: &str,
) -> (PathBuf, PathBuf, PathBuf) {
    let a = fixture.write(
        "a/f.mm",
        &format!(
            "atom f(x: i64) -> i64\n\
             requires: x >= 0;\n\
             ensures: {a_ensures};\n\
             body: x + 1;\n"
        ),
    );
    let b = fixture.write(
        "b/f.mm",
        &format!(
            "atom f(x: i64) -> i64\n\
             requires: x >= 0;\n\
             ensures: {b_ensures};\n\
             body: x;\n"
        ),
    );
    let g = fixture.write(
        "g.mm",
        "import \"a/f.mm\" as lib;\n\
         import \"b/f.mm\" as other;\n\
         atom g(x: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: result >= x;\n\
         body: lib::f(x);\n",
    );
    (a, b, g)
}

#[test]
fn effect_post_change_invalidates_caller_cache() {
    let fixture = Fixture::new();
    let (a, b) = write_effect_project(&fixture, "Open", "Open");
    assert_success(&fixture.verify(&b, &[]), "initial effect-post verification");

    let source = std::fs::read_to_string(&a).expect("read callee source");
    std::fs::write(
        &a,
        source.replace(
            "effect_post: { File: Open }",
            "effect_post: { File: Closed }",
        ),
    )
    .expect("change callee effect_post");
    let changed = fixture.verify(&b, &[]);
    let text = output_text(&changed);
    assert!(!changed.status.success(), "{text}");
    assert!(!text.contains("'g': skipped (unchanged, cached)"), "{text}");
}

#[test]
fn effect_pre_change_invalidates_caller_cache() {
    let fixture = Fixture::new();
    let (a, b) = write_effect_project(&fixture, "Open", "Open");
    assert_success(&fixture.verify(&b, &[]), "initial effect-pre verification");

    let source = std::fs::read_to_string(&a).expect("read callee source");
    std::fs::write(
        &a,
        source.replace("effect_pre: { File: Open }", "effect_pre: { File: Closed }"),
    )
    .expect("change callee effect_pre");
    let changed = fixture.verify(&b, &[]);
    let text = output_text(&changed);
    assert!(!changed.status.success(), "{text}");
    assert!(!text.contains("'g': skipped (unchanged, cached)"), "{text}");
}

#[test]
fn qualified_callee_contract_change_invalidates_caller_cache() {
    let fixture = Fixture::new();
    let (a, _, g) = collision_project(&fixture, "result >= x", "result >= 0");
    assert_success(
        &fixture.verify(&g, &[]),
        "initial qualified-call verification",
    );

    let source = std::fs::read_to_string(&a).expect("read aliased callee source");
    std::fs::write(
        &a,
        source.replace("ensures: result >= x;", "ensures: result >= 0;"),
    )
    .expect("weaken aliased callee contract");
    let changed = fixture.verify(&g, &[]);
    let text = output_text(&changed);
    assert!(!changed.status.success(), "{text}");
    assert!(!text.contains("'g': skipped (unchanged, cached)"), "{text}");
}

#[test]
fn qualified_callee_certificate_reports_qualified_key_deterministically() {
    let fixture = Fixture::new();
    let (a, _, g) = collision_project(&fixture, "result >= x", "result >= 0");
    let cert_path = fixture.root.join("g.proof.json");
    assert_success(&fixture.certify(&g, &cert_path), "certify qualified caller");
    let second_cert_path = fixture.root.join("g-second.proof.json");
    assert_success(
        &fixture.certify(&g, &second_cert_path),
        "certify qualified caller a second time",
    );

    let cert: Value =
        serde_json::from_slice(&std::fs::read(&cert_path).unwrap()).expect("parse certificate");
    let second_cert: Value = serde_json::from_slice(&std::fs::read(&second_cert_path).unwrap())
        .expect("parse second certificate");
    let dependencies = &cert["atoms"][0]["dependency_hashes"];
    assert_eq!(
        dependencies["atom:lib::f"],
        second_cert["atoms"][0]["dependency_hashes"]["atom:lib::f"]
    );
    assert!(
        dependencies.get("atom:other::f").is_none(),
        "{dependencies}"
    );
    assert!(dependencies.get("atom:f").is_none(), "{dependencies}");

    let source = std::fs::read_to_string(&a).expect("read aliased callee source");
    std::fs::write(
        &a,
        source.replace("ensures: result >= x;", "ensures: result >= 0;"),
    )
    .expect("weaken aliased callee contract");
    let changed = fixture.verify_cert(&cert_path, &g, false);
    let text = output_text(&changed);
    assert_eq!(changed.status.code(), Some(1), "{text}");
    assert!(text.contains("g: dependency_changed"), "{text}");
    assert!(text.contains("atom:lib::f"), "{text}");

    let strict = fixture.verify_cert(&cert_path, &g, true);
    let strict_text = output_text(&strict);
    assert_eq!(strict.status.code(), Some(1), "{strict_text}");
    assert!(
        strict_text.contains("g: dependency_changed"),
        "{strict_text}"
    );
    assert!(strict_text.contains("atom:lib::f"), "{strict_text}");
}

#[test]
fn changing_only_same_named_other_import_keeps_caller_proven() {
    let fixture = Fixture::new();
    let (_, b, g) = collision_project(&fixture, "result >= x", "result >= 0");
    let cert_path = fixture.root.join("g.proof.json");
    assert_success(&fixture.certify(&g, &cert_path), "certify qualified caller");

    let source = std::fs::read_to_string(&b).expect("read other callee source");
    std::fs::write(
        &b,
        source.replace("ensures: result >= 0;", "ensures: result >= x;"),
    )
    .expect("strengthen other callee contract");
    let verify = fixture.verify(&g, &[]);
    assert_success(&verify, "verify after unrelated other::f change");

    let checked = fixture.verify_cert(&cert_path, &g, true);
    let text = output_text(&checked);
    assert_eq!(checked.status.code(), Some(0), "{text}");
    assert!(text.contains("g: proven"), "{text}");
}

#[test]
fn impl_method_contract_change_uses_qualified_dependency_key() {
    let fixture = Fixture::new();
    let source = fixture.write(
        "impl.mm",
        "struct Point { x: i64 }\n\
         impl Point {\n\
             atom get(self: Point) -> i64\n\
                 requires: true;\n\
                 ensures: result == self.x;\n\
                 body: self.x;\n\
         }\n\
         atom g(p: Point) -> i64\n\
             requires: p.x >= 0;\n\
             ensures: result == p.x;\n\
             body: Point::get(p);\n",
    );
    assert_success(
        &fixture.verify(&source, &[]),
        "initial impl-method verification",
    );
    let cert_path = fixture.root.join("impl.proof.json");
    assert_success(
        &fixture.certify(&source, &cert_path),
        "certify impl-method caller",
    );

    let original = std::fs::read_to_string(&source).expect("read impl source");
    std::fs::write(
        &source,
        original.replace("ensures: result == self.x;", "ensures: result >= 0;"),
    )
    .expect("weaken impl-method contract");
    let changed = fixture.verify(&source, &[]);
    let text = output_text(&changed);
    assert!(!changed.status.success(), "{text}");
    assert!(!text.contains("'g': skipped (unchanged, cached)"), "{text}");

    let checked = fixture.verify_cert(&cert_path, &source, false);
    let checked_text = output_text(&checked);
    assert_eq!(checked.status.code(), Some(1), "{checked_text}");
    assert!(
        checked_text.contains("g: dependency_changed"),
        "{checked_text}"
    );
    assert!(checked_text.contains("atom:Point::get"), "{checked_text}");
}

#[test]
fn legacy_bare_dependency_key_remains_verifiable_for_aliased_call() {
    let fixture = Fixture::new();
    let a = fixture.write(
        "a/f.mm",
        "atom f(x: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: result >= x;\n\
         body: x;\n",
    );
    let bare = fixture.write(
        "bare.mm",
        "import \"a/f.mm\";\n\
         atom g(x: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: result >= x;\n\
         body: f(x);\n",
    );
    let aliased = fixture.write(
        "aliased.mm",
        "import \"a/f.mm\" as lib;\n\
         atom g(x: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: result >= x;\n\
         body: lib::f(x);\n",
    );
    let bare_cert_path = fixture.root.join("bare.proof.json");
    let alias_cert_path = fixture.root.join("alias.proof.json");
    assert_success(
        &fixture.certify(&bare, &bare_cert_path),
        "certify bare caller",
    );
    assert_success(
        &fixture.certify(&aliased, &alias_cert_path),
        "certify aliased caller",
    );

    let bare_cert: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&bare_cert_path).unwrap())
            .expect("parse bare certificate");
    let bare_hash = bare_cert
        .atoms
        .iter()
        .find(|atom| atom.name == "g")
        .and_then(|atom| atom.dependency_hashes.as_ref())
        .and_then(|hashes| hashes.get("atom:f"))
        .expect("bare dependency hash")
        .clone();
    let mut alias_cert: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&alias_cert_path).unwrap())
            .expect("parse aliased certificate");
    let g = alias_cert
        .atoms
        .iter_mut()
        .find(|atom| atom.name == "g")
        .expect("aliased g certificate");
    g.dependency_hashes = Some(BTreeMap::from([("atom:f".to_string(), bare_hash)]));
    refresh_certificate_integrity(&mut alias_cert);
    std::fs::write(
        &alias_cert_path,
        serde_json::to_vec_pretty(&alias_cert).unwrap(),
    )
    .expect("write legacy-key certificate");

    let checked = fixture.verify_cert(&alias_cert_path, &aliased, true);
    let text = output_text(&checked);
    assert_eq!(checked.status.code(), Some(0), "{text}");
    assert!(text.contains("g: proven"), "{text}");
    assert!(a.exists());
}
