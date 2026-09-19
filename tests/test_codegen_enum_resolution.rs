use std::process::Command;

fn write_fixture(name: &str, source: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mumei_enum_resolution_test_{}_{}",
        name,
        std::process::id()
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale enum-resolution fixture dir");
    }
    std::fs::create_dir_all(&dir).expect("create enum-resolution fixture dir");
    let path = dir.join("main.mm");
    std::fs::write(&path, source).expect("write enum-resolution fixture");
    path
}

/// Emit LLVM IR for `source` and return the module of atom `atom`.
fn emit_atom_ir(name: &str, source: &str, atom: &str) -> String {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let fixture = write_fixture(name, source);
    let dir = fixture.parent().unwrap().to_path_buf();
    let output = Command::new(bin)
        .arg("build")
        .arg(&fixture)
        .arg("--emit")
        .arg("llvm-ir")
        .current_dir(&dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to build fixture {name}: {err}"));
    assert!(
        output.status.success(),
        "build of {name} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let ir = std::fs::read_to_string(dir.join(format!("katana_{atom}.ll")))
        .unwrap_or_else(|err| panic!("no emitted IR for atom {atom}: {err}"));
    std::fs::remove_dir_all(&dir).expect("remove enum-resolution fixture dir");
    ir
}

/// The first `pat_tag_eq` compare's constant — the tag index codegen chose
/// for the match's leading variant arm.
fn first_tag_compare(ir: &str) -> Option<String> {
    let line = ir
        .lines()
        .find(|l| l.contains("pat_tag_eq") && l.contains("icmp eq i64"))?;
    line.rsplit(',').next().map(|c| c.trim().to_string())
}

// `Mine` collides with the auto-loaded prelude `List` on `Cons`:
// `Mine.Cons = 0` while `List.Cons = 1`. The match target's declared type
// (`m: Mine`) must drive the tag — a name-only scan of the `enums` HashMap
// could pick `List` and encode `Cons` as 1, diverging the compiled code
// from what the verifier proved.
#[test]
fn declared_param_type_selects_the_owning_enums_tag_index() {
    let ir = emit_atom_ir(
        "declared_param_owner",
        r#"
enum Mine {
    Cons(i64),
    Nil,
}

atom head_or(m: Mine) -> i64
    requires: true;
    ensures: true;
    body: {
        match m {
            Cons(v) => v
            Nil => 0
        }
    }
"#,
        "head_or",
    );
    assert_eq!(
        first_tag_compare(&ir).as_deref(),
        Some("0"),
        "Cons arm must compare against Mine's tag 0, not prelude List's 1:\n{ir}"
    );
}

// `match th.t` resolves the enum owner through the struct field's declared
// type (`t: E1`), not a HashMap scan: `E1.Cold = 1` collides with
// `E2.Cold = 0`.
#[test]
fn struct_field_match_target_resolves_the_field_enums_tags() {
    let ir = emit_atom_ir(
        "field_target_owner",
        r#"
enum E1 {
    Hot,
    Cold,
}
enum E2 {
    Cold,
    Hot,
}

struct Thermo {
    t: E1,
    label: i64,
}

atom pick(th: Thermo) -> i64
    requires: true;
    ensures: true;
    body: {
        match th.t {
            Hot => 1
            Cold => 0
        }
    }
"#,
        "pick",
    );
    let tags: Vec<String> = ir
        .lines()
        .filter(|l| l.contains("pat_tag_eq") && l.contains("icmp eq i64"))
        .filter_map(|l| l.rsplit(',').next().map(|c| c.trim().to_string()))
        .collect();
    assert_eq!(
        tags,
        vec!["0".to_string(), "1".to_string()],
        "Hot/Cold must use E1's indices (0/1), not E2's (1/0):\n{ir}"
    );
}
