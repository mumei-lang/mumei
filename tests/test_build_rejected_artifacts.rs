//! A source file is accepted or rejected as a whole by `mumei build`: when a
//! later atom fails verification, artifacts already emitted for earlier atoms
//! must not remain on disk.

use std::path::PathBuf;
use std::process::Command;

const MULTI_ATOM_FAIL: &str = r#"
atom take_buffer(buf: [i64])
requires: len(buf) >= 0;
consume buf;
ensures: result >= 0;
body: len(buf);

atom move_buffer_into_two_tasks(buf: [i64])
requires: len(buf) >= 1;
ensures: result >= 0;
body: {
    task_group:all {
        task { take_buffer(buf) };
        task { take_buffer(buf) }
    }
};
"#;

const MULTI_ATOM_PASS: &str = r#"
atom first(x: i64)
requires: x >= 0;
ensures: result >= 0;
body: x;

atom second(y: i64)
requires: y >= 1;
ensures: result >= 1;
body: y;
"#;

fn fixture_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mumei_build_rejected_artifacts_{}_{}",
        name,
        std::process::id()
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale fixture dir");
    }
    std::fs::create_dir_all(&dir).expect("create fixture dir");
    dir
}

fn build(dir: &PathBuf, source: &str, emit: &str) -> std::process::Output {
    let input = dir.join("main.mm");
    std::fs::write(&input, source).expect("write fixture");
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("build")
        .arg(&input)
        .arg("-o")
        .arg(dir.join("out"))
        .arg("--emit")
        .arg(emit)
        .current_dir(dir)
        .output()
        .expect("run mumei build")
}

fn out_artifacts(dir: &PathBuf) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read fixture dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("out"))
        .collect();
    names.sort();
    names
}

#[test]
fn rejected_later_atom_removes_earlier_atom_artifacts() {
    for emit in ["llvm-ir", "c-header", "verified-json"] {
        let dir = fixture_dir(&format!("fail_{emit}"));
        let output = build(&dir, MULTI_ATOM_FAIL, emit);
        assert!(
            !output.status.success(),
            "build with --emit {emit} unexpectedly succeeded"
        );
        let leaked = out_artifacts(&dir);
        assert!(
            leaked.is_empty(),
            "--emit {emit} left artifacts of the accepted atom behind: {leaked:?}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::remove_dir_all(&dir).expect("remove fixture dir");
    }
}

#[test]
fn rejected_build_keeps_artifacts_from_previous_invocations() {
    let dir = fixture_dir("preexisting");
    let stale = dir.join("out_take_buffer.ll");
    std::fs::write(&stale, "; stale artifact from an earlier build\n").expect("write stale");
    // A file whose name does not belong to this build must never be touched.
    let unrelated = dir.join("out_unrelated.txt");
    std::fs::write(&unrelated, "keep me").expect("write unrelated");

    let output = build(&dir, MULTI_ATOM_FAIL, "llvm-ir");
    assert!(!output.status.success());
    // The stale `.ll` shares its path with an artifact this build produced and
    // was overwritten during the run, so it is removed with the rest of the
    // rejected output; unrelated files are left alone.
    assert!(
        !stale.exists(),
        "rejected build must not leave out_take_buffer.ll"
    );
    assert_eq!(
        std::fs::read_to_string(&unrelated).unwrap(),
        "keep me",
        "unrelated file must be untouched"
    );
    std::fs::remove_dir_all(&dir).expect("remove fixture dir");
}

#[test]
fn accepted_multi_atom_build_still_emits_every_artifact() {
    let dir = fixture_dir("pass");
    let output = build(&dir, MULTI_ATOM_PASS, "llvm-ir");
    assert!(
        output.status.success(),
        "build failed\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        out_artifacts(&dir),
        vec!["out_first.ll".to_string(), "out_second.ll".to_string()]
    );
    std::fs::remove_dir_all(&dir).expect("remove fixture dir");
}
