use mumei_core::parser::parse_atom;
use mumei_core::verification::detect_loops_needing_invariants;
use std::process::Command;

#[test]
fn detects_while_loop_for_cegis_suggestion() {
    let atom = parse_atom(
        r#"
atom count_to_n(n: i64) -> i64
requires: n >= 0;
ensures: result == n;
body: {
    let i = 0;
    while i < n invariant: true {
        i = i + 1;
    };
    i
};
"#,
    );

    let loops = detect_loops_needing_invariants(&atom);

    assert_eq!(loops.len(), 1);
    assert!(loops[0].line > 0);
    assert!(loops[0].variables.contains(&"i".to_string()));
    assert!(loops[0].variables.contains(&"n".to_string()));
    assert!(loops[0].needs_invariant);
    assert_eq!(loops[0].context.postcondition, "result == n");
}

#[test]
fn cli_exposes_loop_detection_flags() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let output = Command::new(bin)
        .arg("verify")
        .arg("--help")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run mumei verify --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--detect-loops"));
    assert!(stdout.contains("--suggest-cegis"));
}
