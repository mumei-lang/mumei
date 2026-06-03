use std::process::Command;

fn write_fixture(name: &str, source: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mumei_concurrency_test_{}_{}",
        name,
        std::process::id()
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale concurrency fixture dir");
    }
    std::fs::create_dir_all(&dir).expect("create concurrency fixture dir");
    let path = dir.join("main.mm");
    std::fs::write(&path, source).expect("write concurrency fixture");
    path
}

#[test]
fn task_group_any_cancels_blocked_sibling_after_first_completion() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "task_group_any_cancel",
        r#"
atom main()
requires: true;
ensures: true;
body: {
    task_group:any {
        task { 7 };
        task { recv(0) }
    }
};
"#,
    );

    let output = Command::new(bin)
        .arg("run")
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run task_group:any fixture: {err}"));

    assert_eq!(
        output.status.code(),
        Some(7),
        "task_group:any should return the first completed child and cancel the blocked recv child\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Mumei Run: verify"));
    assert!(stdout.contains("Running"));

    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove concurrency fixture dir");
}
