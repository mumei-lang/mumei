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

// An enum-typed parameter binds the whole tagged-union value (not just the
// tag): `match` extracts the real payload slot, and the full struct is
// forwarded when the parameter is passed to a callee. Before this, enum
// params were split like fat-pointer arrays, so payload bindings read `0`
// and callee forwarding passed the bare tag.
#[test]
fn enum_param_match_binds_payload_and_forwards_struct() {
    let ir = emit_atom_ir(
        "enum_param",
        r#"
enum Mine { Cons(i64), Nil }

atom payload_val(m: Mine) -> i64
    requires: true;
    ensures: true;
    body: {
        match m {
            Mine::Cons(v) => v
            Mine::Nil => 0
        }
    }

atom fwd(m: Mine) -> i64
    requires: true;
    ensures: true;
    body: {
        payload_val(m)
    }
"#,
        "payload_val",
    );
    assert!(
        ir.contains("%variant_payload_0 = extractvalue { i64, i64 } %0, 1"),
        "Cons payload binding must extract the real slot, not 0:\n{ir}"
    );
    let fwd_ir = emit_atom_ir(
        "enum_param_fwd",
        r#"
enum Mine { Cons(i64), Nil }

atom payload_val(m: Mine) -> i64
    requires: true;
    ensures: true;
    body: {
        match m {
            Mine::Cons(v) => v
            Mine::Nil => 0
        }
    }

atom fwd(m: Mine) -> i64
    requires: true;
    ensures: true;
    body: {
        payload_val(m)
    }
"#,
        "fwd",
    );
    assert!(
        fwd_ir.contains("call i64 @payload_val({ i64, i64 } %0)"),
        "enum param must forward the whole struct to the callee:\n{fwd_ir}"
    );
}

// Enum values compare deeply — tag plus every payload slot — matching the
// verifier's datatype equality. `m == Mine::Nil` reduces to a tag compare
// (the unit variant's undef payload slots are skipped), and `a == b`
// between enum-typed parameters compares tag and payload, not just the tag.
#[test]
fn enum_equality_compares_tag_and_payload() {
    let ir = emit_atom_ir(
        "enum_eq",
        r#"
enum Mine { Cons(i64), Nil }

atom eq_ctor(m: Mine) -> i64
    requires: true;
    ensures: true;
    body: {
        if m == Mine::Nil { 1 } else { 0 }
    }

atom eq_params(a: Mine, b: Mine) -> i64
    requires: true;
    ensures: true;
    body: {
        if a == b { 1 } else { 0 }
    }
"#,
        "eq_params",
    );
    assert!(
        ir.contains("enum_eq_int") && ir.matches("extractvalue").count() >= 4,
        "param equality must compare tag AND payload slots:\n{ir}"
    );
}

// Aggregate operands that are not enum values still fail cleanly rather
// than panic inside inkwell's into_int_value.
#[test]
fn equality_on_non_enum_aggregates_errors_cleanly() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let fixture = write_fixture(
        "struct_eq",
        r#"
struct P { x: i64, y: i64 }

atom eq_s(a: P, b: P) -> i64
    requires: true;
    ensures: true;
    body: {
        if a == b { 1 } else { 0 }
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
        "struct equality must be a clean codegen error, not a panic:\n{combined}"
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

// `len(x)` on a non-array value fails closed in both layers: verified atoms
// reject it during type checking, while trusted atoms reject it in codegen.
fn assert_len_on_non_array_fails(name: &str, source: &str, expected: &str) {
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
        .expect("run build");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && combined.contains(expected),
        "len() on an enum value must fail with {expected:?}:\n{combined}"
    );
    std::fs::remove_dir_all(&dir).expect("remove fixture dir");
}

#[test]
fn len_on_non_array_is_a_type_error() {
    assert_len_on_non_array_fails(
        "len_enum_type_error",
        r#"
enum Mine { Cons(i64), Nil }

atom bad_len(m: Mine) -> i64
    requires: true;
    ensures: true;
    body: {
        len(m)
    }
"#,
        "expects an array or string argument",
    );
}

#[test]
fn len_on_non_array_in_trusted_atom_is_a_clean_codegen_error() {
    assert_len_on_non_array_fails(
        "len_enum_codegen_error",
        r#"
enum Mine { Cons(i64), Nil }

trusted atom bad_len(m: Mine) -> i64
    requires: true;
    ensures: true;
    body: {
        len(m)
    }
"#,
        "is only supported on arrays and Str",
    );
}
