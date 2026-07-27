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

// Regression tests for the non-i64 task-capture bug: before the fix,
// `emit_task_spawn_only` only marshalled i64 captures and silently dropped
// every other type, so a captured struct became a zeroed value in the spawned
// thread (its fields were then "not found" / read as zero). These fixtures
// capture an aggregate (`struct`) value into a spawned task and return a field
// from inside the task body — which only yields the expected value if the whole
// captured value is round-tripped through the pthread args struct with its own
// LLVM type. They fail on `develop` (capture dropped -> field not found) and
// pass after the fix.
//
// Scalar (i64/f64) captures are constant-folded before capture analysis, so a
// standalone scalar never reaches the drop path; aggregates are the smallest
// value that genuinely exercises the generalized marshalling here.

#[test]
fn task_spawn_captures_struct_first_field_into_thread_wrapper() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "task_spawn_capture_struct_first",
        r#"
struct Point { x: i64, y: i64 }

trusted atom main()
requires: true;
ensures: true;
body: {
    let p = Point { x: 7, y: 0 };
    task { p.x }
};
"#,
    );

    let output = Command::new(bin)
        .arg("run")
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run struct capture fixture: {err}"));

    assert_eq!(
        output.status.code(),
        Some(7),
        "task spawn should preserve captured struct's first field in the pthread wrapper\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove concurrency fixture dir");
}

#[test]
fn task_spawn_captures_struct_second_field_into_thread_wrapper() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "task_spawn_capture_struct_second",
        r#"
struct Pair { a: i64, b: i64 }

trusted atom main()
requires: true;
ensures: true;
body: {
    let p = Pair { a: 0, b: 7 };
    task { p.b }
};
"#,
    );

    let output = Command::new(bin)
        .arg("run")
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run struct capture fixture: {err}"));

    assert_eq!(
        output.status.code(),
        Some(7),
        "task spawn should preserve captured struct's non-leading field (offset survives) in the pthread wrapper\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove concurrency fixture dir");
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

#[test]
fn task_group_any_nested_leave_restores_outer_cancel_scope() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "task_group_any_nested_leave_scope",
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
            };
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
        .unwrap_or_else(|err| panic!("failed to run nested task_group:any scope fixture: {err}"));

    assert_eq!(
        output.status.code(),
        Some(7),
        "task_group:any cancellation should still be visible after leaving a nested group\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove concurrency fixture dir");
}

// ---------------------------------------------------------------------------
// Structured concurrency ownership (Phase 1h-2)
// ---------------------------------------------------------------------------
// MIR move analysis flattens a `task_group` into a sequential chain of child
// bodies, so it models neither concurrent interleaving nor `task_group:any`
// cancellation. These fixtures pin the AST-level ownership pass that covers
// the patterns beyond `task_group:any` winner cancellation.

fn verify_fixture(name: &str, source: &str) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(name, source);
    let output = Command::new(bin)
        .arg("verify")
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to verify fixture {name}: {err}"));
    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove concurrency fixture dir");
    output
}

fn assert_rejected(output: &std::process::Output, expected_fragment: &str, context: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "{context}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    // Diagnostics are wrapped by miette, so match on a short fragment.
    assert!(
        combined.contains(expected_fragment),
        "expected diagnostic containing {expected_fragment:?}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

const CONSUMING_ATOM: &str = r#"
atom take_buffer(buf: [i64])
requires: len(buf) >= 0;
consume buf;
ensures: result >= 0;
body: len(buf);
"#;

#[test]
fn task_group_all_rejects_concurrent_double_move_of_capture() {
    let source = format!(
        "{CONSUMING_ATOM}
atom move_buffer_into_two_tasks(buf: [i64])
requires: len(buf) >= 1;
ensures: result >= 0;
body: {{
    task_group:all {{
        task {{ take_buffer(buf) }};
        task {{ take_buffer(buf) }}
    }}
}};
"
    );
    let output = verify_fixture("task_group_all_concurrent_double_move", &source);
    assert_rejected(
        &output,
        "concurrent sibling tasks",
        "the same buffer consumed by two concurrent children must be rejected",
    );
}

#[test]
fn task_group_all_rejects_move_while_sibling_still_reads_capture() {
    let source = format!(
        "{CONSUMING_ATOM}
atom move_while_sibling_reads(buf: [i64])
requires: len(buf) >= 1;
ensures: result >= 0;
body: {{
    task_group:all {{
        task {{ take_buffer(buf) }};
        task {{ len(buf) }}
    }}
}};
"
    );
    let output = verify_fixture("task_group_all_move_while_sibling_reads", &source);
    assert_rejected(
        &output,
        "while concurrent sibling",
        "moving a capture a sibling still reads must be rejected",
    );
}

#[test]
fn task_group_all_rejects_unsynchronised_shared_write() {
    let output = verify_fixture(
        "task_group_all_shared_write_race",
        r#"
atom race_on_shared_counter(n: i64)
requires: n >= 0 && n <= 100;
ensures: result >= 0;
body: {
    let counter = n;
    task_group:all {
        task {
            counter = counter + 1;
            counter
        };
        task {
            counter = counter + 2;
            counter
        }
    }
};
"#,
    );
    assert_rejected(
        &output,
        "data race",
        "two siblings writing the same capture must be rejected",
    );
}

#[test]
fn task_group_rejects_parent_use_after_child_moved_capture() {
    let source = format!(
        "{CONSUMING_ATOM}
atom read_buffer_after_task_moved_it(buf: [i64])
requires: len(buf) >= 1;
ensures: result >= 0;
body: {{
    task_group:all {{
        task {{ take_buffer(buf) }};
        task {{ 0 }}
    }};
    len(buf)
}};
"
    );
    let output = verify_fixture("task_group_use_after_concurrent_move", &source);
    assert_rejected(
        &output,
        "moved into a child task",
        "reading a value a child consumed must be rejected",
    );
}

#[test]
fn task_group_any_rejects_parent_read_of_cancellable_write() {
    let output = verify_fixture(
        "task_group_any_cancel_dependent_read",
        r#"
atom read_cancellable_write(n: i64)
requires: n >= 0 && n <= 100;
ensures: result >= 0;
body: {
    let total = n;
    task_group:any {
        task {
            total = total + 1;
            total
        };
        task { n }
    };
    total
};
"#,
    );
    assert_rejected(
        &output,
        "cancellation-dependent value",
        "a value a cancelled child may never have written must be rejected",
    );
}

#[test]
fn task_group_accepts_shared_reads_local_writes_and_single_move() {
    let source = format!(
        "{CONSUMING_ATOM}
atom read_shared_capture_in_siblings(a: i64, b: i64)
requires: a >= 0 && b >= 0;
ensures: result >= 0;
body: {{
    task_group:all {{
        task {{ a + b }};
        task {{ a + b }}
    }}
}};

atom per_task_local_writes(n: i64)
requires: n >= 0 && n <= 100;
ensures: result >= 0;
body: {{
    task_group:all {{
        task {{
            let acc = n;
            acc = acc + 1;
            acc
        }};
        task {{
            let acc = n;
            acc = acc + 2;
            acc
        }}
    }}
}};

atom move_buffer_into_single_task(buf: [i64])
requires: len(buf) >= 1;
ensures: result >= 0;
body: {{
    task_group:all {{
        task {{ take_buffer(buf) }};
        task {{ 0 }}
    }}
}};
"
    );
    let output = verify_fixture("task_group_ownership_positive", &source);
    assert!(
        output.status.success(),
        "safe concurrent captures must keep verifying\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn concurrency_ownership_violation_is_not_a_lean_escalation_candidate() {
    // Ownership violations are decided syntactically: they must fail hard and
    // never become a Z3 `unknown` that the Lean bridge could promote to
    // `lean_verified`.
    let source = format!(
        "{CONSUMING_ATOM}
atom move_buffer_into_two_tasks(buf: [i64])
requires: len(buf) >= 1;
ensures: result >= 0;
body: {{
    task_group:all {{
        task {{ take_buffer(buf) }};
        task {{ take_buffer(buf) }}
    }}
}};
"
    );
    let output = verify_fixture("task_group_ownership_no_lean_escalation", &source);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success());
    assert!(
        combined.contains("0 Lean escalation candidate(s)"),
        "ownership violations must not be escalated to Lean\n{combined}"
    );
    assert!(
        !combined.contains("lean_verified"),
        "ownership violations must never be promoted to lean_verified\n{combined}"
    );
}

const CONSUMING_STRUCT_ATOM: &str = r#"
struct Point { x: i64, y: i64 }

atom take_point(p: Point)
requires: p.x >= 0;
consume p;
ensures: result >= 0;
body: p.x;
"#;

#[test]
fn task_group_rejects_concurrent_double_move_of_struct_capture() {
    let source = format!(
        "{CONSUMING_STRUCT_ATOM}
atom move_point_into_two_tasks(p: Point)
requires: p.x >= 0;
ensures: result >= 0;
body: {{
    task_group:all {{
        task {{ take_point(p) }};
        task {{ take_point(p) }}
    }}
}};
"
    );
    let output = verify_fixture("task_group_struct_double_move", &source);
    assert_rejected(
        &output,
        "concurrent double move",
        "a struct capture consumed by two sibling tasks must be rejected",
    );
}

#[test]
fn task_group_accepts_shared_reads_of_struct_capture() {
    let source = format!(
        "{CONSUMING_STRUCT_ATOM}
atom read_point_in_siblings(p: Point)
requires: p.x >= 0 && p.y >= 0;
ensures: result >= 0;
body: {{
    task_group:all {{
        task {{ p.x }};
        task {{ p.y }}
    }}
}};
"
    );
    let output = verify_fixture("task_group_struct_shared_reads", &source);
    assert!(
        output.status.success(),
        "read-only struct captures must keep verifying\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn json_output_reports_ownership_failure_reason_in_diagnostics() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let source = format!(
        "{CONSUMING_ATOM}
atom move_buffer_into_two_tasks(buf: [i64])
requires: len(buf) >= 1;
ensures: result >= 0;
body: {{
    task_group:all {{
        task {{ take_buffer(buf) }};
        task {{ take_buffer(buf) }}
    }}
}};
"
    );
    let fixture = write_fixture("task_group_ownership_json", &source);
    let output = Command::new(bin)
        .arg("verify")
        .arg(&fixture)
        .arg("--json")
        .current_dir(manifest_dir)
        .output()
        .expect("failed to run verify --json");
    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove concurrency fixture dir");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("no JSON in:\n{stdout}"));
    let payload: serde_json::Value =
        serde_json::from_str(&stdout[json_start..]).expect("verify --json emits valid JSON");

    assert_eq!(payload["status"], "failed");
    let diagnostics = payload["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .clone();
    let failure = diagnostics
        .iter()
        .find(|d| d["atom"] == "move_buffer_into_two_tasks")
        .unwrap_or_else(|| panic!("no failure diagnostic in:\n{payload:#}"));
    assert_eq!(failure["severity"], "error");
    assert_eq!(failure["code"], "failed");
    assert!(
        failure["message"]
            .as_str()
            .unwrap_or_default()
            .contains("concurrent double move"),
        "failure diagnostic must carry the rejection reason: {payload:#}"
    );
}
