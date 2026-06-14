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

    let output = Command::new("timeout")
        .arg("5s")
        .arg(bin)
        .arg("run")
        .arg("tests/test_task_group_any.mm")
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
}

#[test]
fn task_group_any_rejects_postcondition_that_only_later_child_satisfies() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "task_group_any_verifier_result",
        r#"
atom main()
requires: true;
ensures: result == 9;
body: {
    task_group:any {
        task { 7 };
        task {
            recv(0);
            9
        }
    }
};
"#,
    );

    let output = Command::new(bin)
        .arg("verify")
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to verify task_group:any fixture: {err}"));

    assert!(
        !output.status.success(),
        "task_group:any verification must reject a postcondition that does not hold for every possible winning child\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove concurrency fixture dir");
}

#[test]
fn task_group_any_cancels_cpu_loop_sibling_after_first_completion() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "task_group_any_cpu_loop_cancel",
        r#"
trusted atom main()
requires: true;
ensures: true;
body: {
    task_group:any {
        task { 7 };
        task {
            let i = 0;
            while i < 1000000000000
            invariant: i >= 0
            decreases: 1000000000000 - i
            {
                i = i + 1;
            };
            9
        }
    }
};
"#,
    );

    let output = Command::new("timeout")
        .arg("5s")
        .arg(bin)
        .arg("run")
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run task_group:any CPU-loop fixture: {err}"));

    assert_eq!(
        output.status.code(),
        Some(7),
        "task_group:any should return the first completed child and cooperatively cancel the CPU-loop sibling before timeout\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove concurrency fixture dir");
}

#[test]
fn task_group_any_outer_cancel_reaches_nested_task_group() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "task_group_any_nested_cancel",
        r#"
trusted atom main()
requires: true;
ensures: true;
body: {
    task_group:any {
        task { 7 };
        task {
            task_group:any {
                task { recv(0) }
            }
        }
    }
};
"#,
    );

    let output = Command::new("timeout")
        .arg("5s")
        .arg(bin)
        .arg("run")
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run nested task_group:any fixture: {err}"));

    assert_eq!(
        output.status.code(),
        Some(7),
        "outer task_group:any cancellation should propagate through nested task_group:any and avoid hanging on inner recv\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove concurrency fixture dir");
}
