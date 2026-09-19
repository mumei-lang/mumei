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

// Qualified `E::V(..)` / `E::V` constructor expressions lower to VariantInit
// and build the tagged union in place: `Fruit::Apple(n)` inserts tag 0 plus
// the payload, `Fruit::Cherry` (unit) inserts tag 1. Before this, both forms
// failed codegen ("Unknown function" / "Field not found") even though verify
// accepted them.
#[test]
fn qualified_variant_ctor_builds_tagged_union() {
    let ir = emit_atom_ir(
        "qualified_ctor",
        r#"
enum Fruit {
    Apple(i64),
    Cherry,
}

atom bake(n: i64) -> Fruit
    requires: true;
    ensures: true;
    body: {
        if n > 0 { Fruit::Apple(n) } else { Fruit::Cherry }
    }
"#,
        "bake",
    );
    assert!(
        ir.contains("insertvalue { i64, i64 } { i64 0") && ir.contains("i64 %0"),
        "Apple(n) must insert tag 0 plus the param payload:\n{ir}"
    );
    assert!(
        ir.contains("{ i64 1, i64 undef }") || ir.contains("{ i64 1, i64 0 }"),
        "Cherry must produce the tag-1 constant:\n{ir}"
    );
    assert!(
        ir.contains("ret { i64, i64 }"),
        "the function must return the tagged union:\n{ir}"
    );
}

// `Mine::Cons` collides with the prelude `List::Cons` — the qualified ctor
// must still emit Mine's tag (0), never the colliding enum's (1).
#[test]
fn qualified_ctor_under_name_collision_uses_the_named_enum() {
    let ir = emit_atom_ir(
        "qualified_ctor_collision",
        r#"
enum Mine {
    Cons(i64),
    Nil,
}

atom mk(p: i64) -> Mine
    requires: true;
    ensures: true;
    body: {
        Mine::Cons(p)
    }
"#,
        "mk",
    );
    assert!(
        ir.contains("insertvalue { i64, i64 } { i64 0"),
        "Mine::Cons must insert Mine's tag 0, not prelude List's 1:\n{ir}"
    );
}

// `let m = Mine::Nil` binds `m: Mine` — the unit-variant FieldAccess records
// its enum in `var_types`, so the following `match m` resolves Mine's tags
// (Cons = 0) even though the prelude `List::Cons` collides.
#[test]
fn let_bound_unit_ctor_types_the_owner_for_match() {
    let ir = emit_atom_ir(
        "let_bound_ctor",
        r#"
enum Mine {
    Cons(i64),
    Nil,
}

atom mk_nullary() -> i64
    requires: true;
    ensures: true;
    body: {
        let m = Mine::Nil;
        match m {
            Cons(v) => v
            Nil => 7
        }
    }
"#,
        "mk_nullary",
    );
    // The constructed tag is constant, so the tag compares fold — the Nil
    // arm (Mine tag 1) must be the statically-selected branch returning 7.
    assert!(
        ir.contains("br i1 true, label %match.body_1"),
        "Nil arm must be selected for the tag-1 constant:\n{ir}"
    );
    assert!(
        ir.contains("[ 7, %match.body_1 ]"),
        "Nil arm must contribute 7 to the match result phi:\n{ir}"
    );
}

// A constructed enum value can flow into a `match` target and into another
// atom's enum-typed parameter.
#[test]
fn ctor_value_flows_into_match_and_callee() {
    let ir = emit_atom_ir(
        "ctor_flow",
        r#"
enum Mine { Cons(i64), Nil }

atom consume(m: Mine) -> i64
    requires: true;
    ensures: true;
    body: {
        match m {
            Mine::Cons(v) => v
            Mine::Nil => 0
        }
    }

atom both(n: i64) -> i64
    requires: true;
    ensures: true;
    body: {
        let a = match Mine::Cons(n) {
            Mine::Cons(v) => v
            Mine::Nil => 0
        };
        consume(Mine::Cons(a))
    }
"#,
        "both",
    );
    assert!(
        ir.contains("call i64 @consume") && ir.contains("insertvalue { i64, i64 }"),
        "ctor value must be passed to the callee's enum param:\n{ir}"
    );
}

// Binary operators on aggregate values used to panic inside inkwell's
// into_int_value — they now fail with a clean codegen error instead.
#[test]
fn equality_on_enum_values_errors_cleanly() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let fixture = write_fixture(
        "enum_eq",
        r#"
enum Mine { Cons(i64), Nil }

atom eq_ctor(m: Mine) -> i64
    requires: true;
    ensures: true;
    body: {
        if m == Mine::Nil { 1 } else { 0 }
    }
"#,
    );
    let dir = fixture.parent().unwrap().to_path_buf();
    let output = Command::new(bin)
        .arg("build")
        .arg(&fixture)
        .arg("--emit")
        .arg("llvm-ir")
        .current_dir(&dir)
        .output()
        .expect("run build");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && combined.contains("unsupported on struct/enum values"),
        "enum equality must be a clean codegen error, not a panic:\n{combined}"
    );
    std::fs::remove_dir_all(&dir).expect("remove fixture dir");
}

// A recursive enum cannot be laid out eagerly — construction fails with a
// clean codegen error instead of overflowing the stack in enum_llvm_type.
#[test]
fn recursive_enum_ctor_fails_with_codegen_error() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let fixture = write_fixture(
        "recursive_ctor",
        r#"
enum IntList {
    Cons(i64, IntList),
    Nil,
}

atom mk() -> i64
    requires: true;
    ensures: true;
    body: {
        let l = IntList::Nil;
        0
    }
"#,
    );
    let dir = fixture.parent().unwrap().to_path_buf();
    let output = Command::new(bin)
        .arg("build")
        .arg(&fixture)
        .arg("--emit")
        .arg("llvm-ir")
        .current_dir(&dir)
        .output()
        .expect("run build");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && combined.contains("recursive"),
        "recursive enum ctor must fail with a clean codegen error:\n{combined}"
    );
    std::fs::remove_dir_all(&dir).expect("remove fixture dir");
}
