use mumei_core::hir::lower_atom_to_hir;
use mumei_core::parser::{parse_module_checked, Item};
use mumei_core::verification::ModuleEnv;
use mumei_emit_llvm::translation_validation::{self as tv, Options, Outcome};

const CLAMP: &str = include_str!("fixtures/semantic_contract_clamp.mm");

fn pin(ir: &str) -> String {
    format!(
        "target triple = \"{}\"\ntarget datalayout = \"{}\"\n{ir}",
        tv::TARGET,
        tv::DATA_LAYOUT
    )
}

fn source(params: &str, body: &str) -> String {
    format!("atom f({params}) -> i64 requires: true; ensures: true; body: {{ {body} }};")
}

fn emitted(source: &str) -> String {
    let Item::Atom(atom) = parse_module_checked(source).unwrap().remove(0) else {
        panic!("atom")
    };
    let hir = lower_atom_to_hir(&atom);
    pin(
        &mumei_emit_llvm::codegen::compile_to_module(&hir.emit_view(), &ModuleEnv::default(), &[])
            .unwrap(),
    )
}

fn validate(source: &str, ir: &str) -> tv::ValidationReport {
    tv::validate_pure_i64(source, ir, Options::default()).unwrap()
}

fn function(body: &str) -> String {
    pin(&format!("define i64 @f(i64 %x) {{ {body} }}"))
}

#[test]
fn actual_emitter_clamp_is_equivalent_over_all_i64_inputs() {
    let ir = emitted(CLAMP);
    let report = validate(CLAMP, &ir);
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    assert_eq!(report.profile, tv::PROFILE);
    assert_eq!(report.target, tv::TARGET);
    assert_eq!(report.data_layout, tv::DATA_LAYOUT);
    assert_eq!(
        report.checked_stage,
        "supplied_llvm_ir_snapshot_no_pipeline_attestation"
    );
    assert_eq!(
        report.input_domain,
        "all_signed_i64_inputs_not_restricted_by_requires"
    );
    assert_eq!(
        report.source_sha256,
        mumei_core::native_bundle::sha256(CLAMP.as_bytes())
    );
    assert_eq!(
        report.llvm_sha256,
        mumei_core::native_bundle::sha256(ir.as_bytes())
    );
}

#[test]
fn real_emitter_comparison_boolean_and_nested_branch_matrix() {
    for op in ["<", "<=", ">", ">=", "==", "!="] {
        for params in ["x: i64", "x: i64, y: i64"] {
            let rhs = if params.contains('y') { "y" } else { "0" };
            for condition in [
                format!("x {op} {rhs}"),
                format!("(x {op} {rhs}) && (x >= 0)"),
                format!("(x {op} {rhs}) || (x >= 0)"),
                format!("(x {op} {rhs}) == (x >= 0)"),
                format!("if x > 0 {{ x {op} {rhs} }} else {{ false }}"),
            ] {
                let src = source(
                    params,
                    &format!("if {condition} {{ if x == 7 {{ 99 }} else {{ x }} }} else {{ 42 }}"),
                );
                assert_eq!(
                    validate(&src, &emitted(&src)).outcome,
                    Outcome::EquivalentWithinProfile,
                    "{condition}"
                );
            }
        }
    }
    for body in [
        "x",
        "0",
        "9223372036854775807",
        "if true { x } else { 7 }",
        "if false { 7 } else { x }",
    ] {
        let src = source("x: i64", body);
        assert_eq!(
            validate(&src, &emitted(&src)).outcome,
            Outcome::EquivalentWithinProfile
        );
    }
    let src = source("", "42");
    assert_eq!(
        validate(&src, &emitted(&src)).outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn wrong_comparisons_unsigned_lowering_and_branch_phi_replacements_are_refuted() {
    let ir = emitted(CLAMP);
    for replacement in ["sgt", "sge", "eq", "ne", "ult", "ugt"] {
        let wrong = ir.replace("icmp slt", &format!("icmp {replacement}"));
        assert_ne!(wrong, ir);
        assert_eq!(
            validate(CLAMP, &wrong).outcome,
            Outcome::Different,
            "{replacement}"
        );
    }
    // At zero both clamp branches yield zero; textual difference is not inequivalence.
    assert_eq!(
        validate(CLAMP, &ir.replace("icmp slt", "icmp sle")).outcome,
        Outcome::EquivalentWithinProfile
    );
    assert_eq!(
        validate(
            CLAMP,
            &pin("define i64 @clamp_nonnegative(i64 %x) { ret i64 %x }")
        )
        .outcome,
        Outcome::Different
    );
    assert_eq!(
        validate(
            CLAMP,
            &pin("define i64 @clamp_nonnegative(i64 %x) { ret i64 0 }")
        )
        .outcome,
        Outcome::Different
    );
}

#[test]
fn equivalence_does_not_prove_contracts_or_accept_false_preconditions_vacuously() {
    let ir = emitted(CLAMP);
    let false_post = CLAMP.replace("(x < 0 => result == 0) && (x >= 0 => result == x)", "false");
    let false_pre = CLAMP.replace("requires: true", "requires: false");
    assert_eq!(
        validate(&false_post, &ir).outcome,
        Outcome::EquivalentWithinProfile
    );
    assert_eq!(
        validate(&false_pre, &ir.replace("icmp slt", "icmp sgt")).outcome,
        Outcome::Different
    );
    assert_ne!(
        validate(CLAMP, &ir).source_sha256,
        validate(&false_post, &ir).source_sha256
    );
}

#[test]
fn argument_order_is_positional_not_llvm_identifier_spelling() {
    let src = source("x: i64, y: i64", "x");
    let good = pin("define i64 @f(i64 %y, i64 %x) { ret i64 %y }");
    let wrong = pin("define i64 @f(i64 %y, i64 %x) { ret i64 %x }");
    assert_eq!(
        validate(&src, &good).outcome,
        Outcome::EquivalentWithinProfile
    );
    assert_eq!(validate(&src, &wrong).outcome, Outcome::Different);
}

#[test]
fn differences_on_rare_and_signed_boundary_inputs_are_not_finite_replay() {
    let src = source("x: i64", "x");
    for value in ["123456789", "-9223372036854775808", "9223372036854775807"] {
        let ir = function(&format!(
            "%c = icmp eq i64 %x, {value}\n%v = select i1 %c, i64 0, i64 %x\nret i64 %v"
        ));
        assert_eq!(validate(&src, &ir).outcome, Outcome::Different);
    }
}

#[test]
fn selection_cast_and_bitwise_ssa_encodings_are_checked_independently() {
    let src = source("x: i64", "if x < 0 { 0 } else { x }");
    for body in [
        "%c = icmp slt i64 %x, 0\n%v = select i1 %c, i64 0, i64 %x\nret i64 %v",
        "%c = icmp slt i64 %x, 0\n%z = zext i1 %c to i64\n%t = trunc i64 %z to i1\n%a = and i1 %t, true\n%o = or i1 %a, false\n%n = xor i1 %o, false\n%v = select i1 %n, i64 0, i64 %x\nret i64 %v",
    ] {
        assert_eq!(validate(&src, &function(body)).outcome, Outcome::EquivalentWithinProfile);
    }
    let wrong = function("%v = and i64 %x, 9223372036854775807\nret i64 %v");
    assert_eq!(validate(&src, &wrong).outcome, Outcome::Different);
}

#[test]
fn source_implication_is_checked_without_claiming_emitter_support() {
    let src = source("x: i64", "if (x < 0) => (x > 0) { 1 } else { x }");
    let ir = function("%a = icmp slt i64 %x, 0\n%b = icmp sgt i64 %x, 0\n%n = xor i1 %a, true\n%c = or i1 %n, %b\n%v = select i1 %c, i64 1, i64 %x\nret i64 %v");
    assert_eq!(
        validate(&src, &ir).outcome,
        Outcome::EquivalentWithinProfile
    );
    let wrong = ir.replace("xor i1 %a, true", "xor i1 %a, false");
    assert_eq!(validate(&src, &wrong).outcome, Outcome::Different);
    let Item::Atom(atom) = parse_module_checked(&src).unwrap().remove(0) else {
        panic!("atom")
    };
    let hir = lower_atom_to_hir(&atom);
    assert!(mumei_emit_llvm::codegen::compile_to_module(
        &hir.emit_view(),
        &ModuleEnv::default(),
        &[]
    )
    .is_err());
}

#[test]
fn multiple_returns_and_phi_predecessors_are_not_merged_by_value_name() {
    let src = source("x: i64", "if x < 0 { 0 } else { x }");
    let ir = function("entry: %c = icmp slt i64 %x, 0\nbr i1 %c, label %neg, label %pos\nneg: ret i64 0\npos: ret i64 %x");
    assert_eq!(
        validate(&src, &ir).outcome,
        Outcome::EquivalentWithinProfile
    );
    let ir = function("entry: %c = icmp slt i64 %x, 0\nbr i1 %c, label %neg, label %pos\nneg: br label %end\npos: br label %end\nend: %p = phi i64 [0, %neg], [%x, %pos]\nret i64 %p");
    assert_eq!(
        validate(&src, &ir).outcome,
        Outcome::EquivalentWithinProfile
    );
    let wrong = ir.replace("[0, %neg], [%x, %pos]", "[%x, %neg], [0, %pos]");
    assert_eq!(validate(&src, &wrong).outcome, Outcome::Different);
}

#[test]
fn unsupported_source_is_not_silently_weakened() {
    for body in [
        "x**2",
        "1.0",
        "f(x)",
        "while x > 0 invariant true { x }; x",
        // Sequences admit `let`s, scalar reassignments and pure
        // expression statements: an unbound assignment, a call statement
        // and a trailing `let` without a value still fail.
        "x = 1",
        "y = 1; x",
        "f(x); x",
        "let y = x;",
        "let y = f(x); y",
        // Rebinding cannot escape a conditional path or a nested block:
        // assignments inside `if` branches or nested blocks may only
        // touch names bound within them.
        "if x > 0 { x = 1; 0 } else { 0 }",
        "let a = x; if x > 0 { a = 1; a } else { a }",
        "let a = x; { a = 1; a }; a",
        // Reassignment keeps the binding's kind.
        "let a = x; a = x > 0; a",
    ] {
        assert!(tv::validate_pure_i64(
            &source("x: i64", body),
            &function("ret i64 %x"),
            Options::default()
        )
        .is_err());
    }
    // Boolean parameters carry truth values that are not i64 operands, a
    // body must still return i64, and reassigning a bool to a non-bool
    // value keeps the binding's kind.
    for (params, body) in [
        ("b: bool", "b + 1"),
        ("b: bool", "b"),
        ("b: bool", "if b { b } else { b }"),
        ("b: bool, x: i64", "b = 1; x"),
    ] {
        assert!(tv::validate_pure_i64(
            &source(params, body),
            &function("ret i64 0"),
            Options::default()
        )
        .is_err());
    }
    // An i64 IR parameter is signature-compatible with a declared bool
    // (both lower to i64) but not interchangeable: ignoring the boolean
    // input is a value difference on the restricted domain.
    let bool_src = source("b: bool, x: i64", "if b { x } else { 0 }");
    assert_eq!(
        validate(
            &bool_src,
            &pin("define i64 @f(i64 %b, i64 %x) { ret i64 %x }")
        )
        .outcome,
        Outcome::Different
    );
    // An i1 IR parameter is a different signature than the i64 0/1 a
    // declared bool lowers to — a signature error, not a value difference.
    assert!(tv::validate_pure_i64(
        &bool_src,
        &pin("define i64 @f(i1 %b, i64 %x) { ret i64 %x }"),
        Options::default()
    )
    .is_err());
    for params in ["x: u64", "x: i32", "x: [i64]", "x: f64"] {
        assert!(tv::validate_pure_i64(
            &source(params, "0"),
            &function("ret i64 0"),
            Options::default()
        )
        .is_err());
    }
}

#[test]
fn bool_parameters_are_equivalent_and_refuted() {
    for (params, body) in [
        // A `bool` parameter lowers to i64 carrying 0/1, usable in guards
        // and boolean expressions; these cases keep an i64 result.
        ("b: bool, x: i64", "if b { x } else { 0 }"),
        ("b: bool", "if b { 1 } else { 0 }"),
        ("b: bool, x: i64", "let c = b; if c { x } else { x }"),
        ("b: bool, x: i64", "if b == true { x } else { 0 }"),
        ("b: bool, x: i64", "if (x > 0) == b { x } else { 0 }"),
        ("b: bool, x: i64", "b = x < 0; if b { x } else { 0 }"),
        ("b: bool, x: i64", "if b { x + 1; x } else { 0 }"),
        ("b: bool, c: bool, x: i64", "if b && c { x } else { 0 }"),
        (
            "b: bool, x: i64",
            "let c = b || x > 0; if c { x } else { 1 }",
        ),
        ("b: bool, x: i64", "if !b { x } else { 0 }"),
        // A boolean guard over a partial operation keeps both domains:
        // the 0/1 signature bound and the operation's definedness.
        ("b: bool, x: i64, y: i64", "if b { x / y } else { 0 }"),
    ] {
        let src = source(params, body);
        assert_eq!(
            validate(&src, &emitted(&src)).outcome,
            Outcome::EquivalentWithinProfile,
            "{body}"
        );
    }
    // A `requires` on a boolean input is a source-side obligation; the
    // comparison domain still covers every input the signature admits.
    let req = format!(
        "atom f(b: bool, x: i64) -> i64 requires: b; ensures: true; body: {{ if b {{ x }} else {{ 0 }} }};"
    );
    assert_eq!(
        validate(&req, &emitted(&req)).outcome,
        Outcome::EquivalentWithinProfile
    );
    // The domain label discloses the 0/1-restricted bool signature.
    let src = source("b: bool, x: i64", "if b { x } else { 0 }");
    let report = validate(&src, &emitted(&src));
    assert_eq!(
        report.input_domain,
        "signed_i64_inputs_with_bool_params_restricted_to_0_or_1_not_restricted_by_requires"
    );
    // Ignoring the boolean input or swapping its branches is different.
    assert_eq!(
        validate(&src, &pin("define i64 @f(i64 %b, i64 %x) { ret i64 %x }")).outcome,
        Outcome::Different
    );
    let swapped = emitted(&source("b: bool, x: i64", "if b { 0 } else { x }"));
    assert_eq!(validate(&src, &swapped).outcome, Outcome::Different);
    // Equivalent reorderings still compare equal.
    let same = emitted(&source("b: bool, x: i64", "if b == false { 0 } else { x }"));
    assert_eq!(
        validate(&src, &same).outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn bool_returns_are_equivalent_and_refuted() {
    // A `-> bool` result lowers to i64 carrying 0/1 (the emitter zexts
    // its i1), so the IR return stays in the i64 profile.
    for src in [
        "atom f(b: bool) -> bool requires: true; ensures: true; body: { b };",
        "atom f(x: i64) -> bool requires: true; ensures: true; body: { x > 0 };",
        "atom f(x: i64, y: i64) -> bool requires: true; ensures: true; body: { x > y };",
        "atom f(b: bool, c: bool) -> bool requires: true; ensures: true; body: { b && c };",
        "atom f(b: bool) -> bool requires: true; ensures: true; body: { !b };",
        "atom f(b: bool, x: i64) -> bool requires: true; ensures: true; body: { if b { x > 0 } else { false } };",
        "atom f(x: i64) -> bool requires: true; ensures: true; body: { let p = x > 0; p };",
        "atom f(x: i64) -> bool requires: true; ensures: true; body: { let p = x > 0; p = !p; p };",
        "atom f(x: i64, y: i64) -> bool requires: true; ensures: true; body: { x / y > 0 };",
        "atom f(x: i64) -> bool requires: x != 0; ensures: result == (x > 0); body: { x > 0 };",
    ] {
        assert_eq!(
            validate(src, &emitted(src)).outcome,
            Outcome::EquivalentWithinProfile,
            "{src}"
        );
    }
    // A wrong truth value or a value the signature cannot carry is a
    // real difference, not a signature error: `ret i64 5` is i64, but
    // it is not the 0/1 a declared `-> bool` admits.
    let src = "atom f(x: i64) -> bool requires: true; ensures: true; body: { x > 0 };";
    for ir in [
        pin("define i64 @f(i64 %x) { ret i64 %x }"),
        pin("define i64 @f(i64 %x) { ret i64 5 }"),
        pin("define i64 @f(i64 %x) { ret i64 0 }"),
    ] {
        assert_eq!(validate(src, &ir).outcome, Outcome::Different, "{ir}");
    }
    // An i1 return is a different signature than the i64 0/1 the
    // declared `-> bool` lowers to — a signature error.
    assert!(tv::validate_pure_i64(
        src,
        &pin("define i1 @f(i64 %x) { ret i1 0 }"),
        Options::default()
    )
    .is_err());
    // Equivalent reorderings still compare equal.
    let same =
        emitted("atom f(x: i64) -> bool requires: true; ensures: true; body: { !(x <= 0) };");
    assert_eq!(
        validate(src, &same).outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn bool_returns_stay_typed() {
    // A `-> bool` body must produce a Bool kind; an i64 body is rejected.
    for src in [
        "atom f(x: i64) -> bool requires: true; ensures: true; body: { x };",
        "atom f(b: bool) -> bool requires: true; ensures: true; body: { if b { 1 } else { 0 } };",
        "atom f(x: i64) -> u64 requires: true; ensures: true; body: { x };",
        // `result` in ensures has the declared Bool kind, not i64.
        "atom f(x: i64) -> bool requires: true; ensures: result + 1 > 0; body: { x > 0 };",
    ] {
        assert!(tv::validate_pure_i64(src, &function("ret i64 0"), Options::default()).is_err());
    }
}

#[test]
fn wrapping_i64_arithmetic_is_equivalent_and_refuted() {
    for (params, body) in [
        ("x: i64", "x + 1"),
        ("x: i64", "x - 1"),
        ("x: i64", "x * 2"),
        ("x: i64, y: i64", "x + y"),
        ("x: i64, y: i64", "x - y"),
        ("x: i64, y: i64", "x * y"),
        ("x: i64, y: i64", "x * x - y"),
        ("x: i64, y: i64", "(x + y) * (x - y)"),
        ("x: i64", "if x + 1 > 0 { x * 2 } else { 0 - x }"),
        // Wrap: addition of a negative literal lowers to `sub`; -x to `0 - x`.
        ("x: i64", "x + 0 - 1"),
        // Bitwise ops are total on i64 — no definedness obligation needed.
        ("x: i64", "x & 3"),
        ("x: i64", "x | 5"),
        ("x: i64", "x ^ x"),
        ("x: i64, y: i64", "x & y"),
        ("x: i64, y: i64", "x | y"),
        ("x: i64, y: i64", "x ^ y"),
        ("x: i64, y: i64", "(x & y) | (x ^ y)"),
        ("x: i64", "if x & 1 == 0 { x | 2 } else { x ^ 3 }"),
    ] {
        let src = source(params, body);
        assert_eq!(
            validate(&src, &emitted(&src)).outcome,
            Outcome::EquivalentWithinProfile,
            "{body}"
        );
    }
    // Nonlinear cases that outrun the solver budget fail closed as
    // inconclusive — never weakened into equivalence or difference.
    let guarded = source("x: i64", "if x + 1 > 0 { x * x } else { 0 - x }");
    assert_eq!(
        validate(&guarded, &emitted(&guarded)).outcome,
        Outcome::Inconclusive
    );
    // Deliberately wrong lowering is detected, including rare-input diffs.
    let src = source("x: i64, y: i64", "x + y");
    let ir = emitted(&src);
    for wrong in [
        ir.replace("add i64", "sub i64"),
        ir.replace("add i64 %0, %1", "add i64 %0, 1"),
        ir.replace("add i64 %0, %1", "add i64 %1, 0"),
    ] {
        assert_ne!(wrong, ir);
        assert_eq!(
            validate(&src, &wrong).outcome,
            Outcome::Different,
            "{wrong}"
        );
    }
    // x*2 and x+x are equal for every input — a textual difference is not a bug.
    let doubled = source("x: i64", "x + x");
    assert_eq!(
        validate(&doubled, &emitted(&source("x: i64", "x * 2"))).outcome,
        Outcome::EquivalentWithinProfile
    );
    // A wrong bitwise opcode or operand is a genuine difference, not noise.
    let masked = source("x: i64", "x & 3");
    for wrong in [
        pin("define i64 @f(i64 %x) { %v = or i64 %x, 3\nret i64 %v }"),
        pin("define i64 @f(i64 %x) { %v = xor i64 %x, 3\nret i64 %v }"),
        pin("define i64 @f(i64 %x) { %v = and i64 %x, 7\nret i64 %v }"),
    ] {
        assert_eq!(
            validate(&masked, &wrong).outcome,
            Outcome::Different,
            "{wrong}"
        );
    }
    // `x & x` and `x | x` are both just `x` — cross-opcode equality still holds.
    let self_and = source("x: i64", "x & x");
    assert_eq!(
        validate(&self_and, &emitted(&source("x: i64", "x | x"))).outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn lexical_let_bindings_are_equivalent_scoped_and_refuted() {
    for (params, body) in [
        ("x: i64", "let y = x; y"),
        ("x: i64", "let y = x + 1; y * 2"),
        ("x: i64", "let x = x + 1; x"),
        ("x: i64, y: i64", "let a = x & y; let b = a | 1; b"),
        ("x: i64", "let b = x > 0; if b { x } else { 0 }"),
        ("x: i64", "if x > 0 { let y = x; y + 1 } else { 0 }"),
        ("x: i64", "let result = x; result"),
    ] {
        let src = source(params, body);
        assert_eq!(
            validate(&src, &emitted(&src)).outcome,
            Outcome::EquivalentWithinProfile,
            "{body}"
        );
    }
    // A `let` initializer with partial operations keeps its definedness
    // obligations under the guarding condition — the domain is unchanged.
    for body in ["let d = x / y; d", "let s = x << y; s"] {
        let src = source("x: i64, y: i64", body);
        let report = validate(&src, &emitted(&src));
        assert_eq!(report.outcome, Outcome::EquivalentWithinProfile, "{body}");
        assert_eq!(
            report.input_domain,
            "signed_i64_inputs_where_division_and_shift_defined_not_restricted_by_requires",
            "{body}"
        );
    }
    // `let y = x; y` returns x, not x + 1 — the binding is not a rewrite.
    let src = source("x: i64", "let y = x; y");
    let wrong = emitted(&source("x: i64", "x + 1"));
    assert_eq!(validate(&src, &wrong).outcome, Outcome::Different);
}

#[test]
fn pure_expression_statements_are_equivalent_and_refuted() {
    for (params, body) in [
        // A discarded expression evaluates for its obligations only —
        // the value statement still yields the result.
        ("x: i64", "x + 1; x"),
        ("x: i64", "x & 3; x | 2"),
        ("x: i64", "x > 0; x"),
        ("x: i64", "let a = x; a + 1; a"),
        ("x: i64", "let a = x + 1; let b = a * 2; b - a; b"),
        ("x: i64", "if x > 0 { 1 } else { 2 }; x"),
    ] {
        let src = source(params, body);
        assert_eq!(
            validate(&src, &emitted(&src)).outcome,
            Outcome::EquivalentWithinProfile,
            "{body}"
        );
    }
    // A discarded partial operation keeps its definedness obligation:
    // `x / y; x` is defined exactly where the division is.
    let src = source("x: i64, y: i64", "x / y; x");
    let report = validate(&src, &emitted(&src));
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    assert_eq!(
        report.input_domain,
        "signed_i64_inputs_where_division_and_shift_defined_not_restricted_by_requires"
    );
    // `x + 1; x` returns x — the discarded expression is not a rewrite.
    let src = source("x: i64", "x + 1; x");
    let wrong = emitted(&source("x: i64", "x + 1"));
    assert_eq!(validate(&src, &wrong).outcome, Outcome::Different);
}

#[test]
fn scalar_reassignment_is_equivalent_and_refuted() {
    for (params, body) in [
        // Rebinding a parameter or a block `let` updates the value the
        // tail statement yields.
        ("x: i64", "x = 1; x"),
        ("x: i64", "x = x + 1; x"),
        ("x: i64", "let a = x; a = a + 1; a"),
        ("x: i64", "let a = x; a = x + 1; a = a * 2; a"),
        ("x: i64", "let b = x > 0; b = x < 0; if b { x } else { 0 }"),
        // A branch-local binding may be reassigned inside its own branch.
        ("x: i64", "if x > 0 { let a = x; a = a + 1; a } else { 0 }"),
        // Mixed with discarded expression statements.
        ("x: i64", "let a = x; a + 1; a = a * 2; a"),
        // A nested block keeps its bindings lexical and discards its
        // value: `let a` inside shadows, reads of the outer `a` still see
        // the enclosing binding.
        ("x: i64", "let a = x; { let b = a + 1; b }; a"),
        ("x: i64", "let a = x; { let a = a + 1; a }; a"),
        ("x: i64", "let a = x; { let b = a; { b + 1; b } }; a"),
        // A nested block may reassign its own bindings.
        ("x: i64", "let a = x; { let b = a; b = b * 2; b }; a"),
    ] {
        let src = source(params, body);
        assert_eq!(
            validate(&src, &emitted(&src)).outcome,
            Outcome::EquivalentWithinProfile,
            "{body}"
        );
    }
    // A reassigned partial operation keeps its definedness obligation:
    // `x = x / y; x` is defined exactly where the division is.
    let src = source("x: i64, y: i64", "x = x / y; x");
    let report = validate(&src, &emitted(&src));
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    assert_eq!(
        report.input_domain,
        "signed_i64_inputs_where_division_and_shift_defined_not_restricted_by_requires"
    );
    // `x = x + 1; x` returns x + 1 — not x + 2 or the original x.
    let src = source("x: i64", "x = x + 1; x");
    for other in ["x + 2", "x"] {
        let wrong = emitted(&source("x: i64", other));
        assert_eq!(
            validate(&src, &wrong).outcome,
            Outcome::Different,
            "{other}"
        );
    }
}

#[test]
fn ssa_names_containing_flag_substrings_are_not_wrap_flags() {
    // `%answer`/`%nuwrap` contain nsw/nuw as substrings but carry no wrap flag.
    let src = source("x: i64", "x + 1");
    let ir = pin(
        "define i64 @f(i64 %x) { %answer = add i64 %x, 1\n%nuwrap = add i64 %answer, 0\nret i64 %nuwrap }",
    );
    assert_eq!(
        validate(&src, &ir).outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn guarded_i64_division_and_shifts_are_equivalent_or_refuted() {
    for (params, body) in [
        ("x: i64, y: i64", "x / y"),
        ("x: i64, y: i64", "x << y"),
        ("x: i64, y: i64", "x >> y"),
        ("x: i64", "x / 4"),
        ("x: i64", "x % 7"),
        ("x: i64, y: i64", "x / (y + 1 - y)"),
        ("x: i64", "x << 3"),
        ("x: i64", "x >> 5"),
        // Guarded in source: the divisor is provably nonzero on the path
        // that divides, so both sides share the same partial domain.
        ("x: i64, y: i64", "if y != 0 { x / y } else { 0 }"),
        ("x: i64, y: i64", "if y < 64 { x << y } else { 0 }"),
        // `&&`/`||` short-circuit: the division on the right binds only
        // where its left operand lets it run — as in `--bitvec-i64`.
        ("x: i64, y: i64", "if y != 0 && x / y == 0 { 1 } else { 0 }"),
        ("x: i64, y: i64", "if y == 0 || x / y != 0 { 1 } else { 0 }"),
    ] {
        let src = source(params, body);
        let report = validate(&src, &emitted(&src));
        assert_eq!(report.outcome, Outcome::EquivalentWithinProfile, "{body}");
        assert_eq!(
            report.input_domain,
            "signed_i64_inputs_where_division_and_shift_defined_not_restricted_by_requires",
            "{body}"
        );
    }
    // A variable-divisor `srem` — emitted as `srem x, select(y == -1, 1, y)` —
    // outruns the bounded solver budget (inconclusive even at 30s/4G rlimit).
    // The contract fails closed rather than weakening the claim.
    for body in ["x % y", "if y == 0 { 0 } else { x % y }"] {
        let src = source("x: i64, y: i64", body);
        assert_eq!(
            validate(&src, &emitted(&src)).outcome,
            Outcome::Inconclusive,
            "{body}"
        );
    }
    // Deliberately wrong lowering on the shared domain is still refuted —
    // never reported equivalent. Nonlinear wrong variants (a variable-divisor
    // `srem`, swapped operands) outrun the budget and fail closed as
    // inconclusive, which is still not a proof of equivalence.
    let src = source("x: i64, y: i64", "x / y");
    let ir = emitted(&src);
    let wrong_const = ir.replace("sdiv i64 %0, %1", "sdiv i64 %0, 2");
    assert_ne!(wrong_const, ir);
    assert_eq!(validate(&src, &wrong_const).outcome, Outcome::Different);
    for wrong in [
        ir.replace("sdiv i64", "srem i64"),
        ir.replace("sdiv i64 %0, %1", "sdiv i64 %1, %0"),
    ] {
        assert_ne!(wrong, ir);
        assert_ne!(
            validate(&src, &wrong).outcome,
            Outcome::EquivalentWithinProfile,
            "{wrong}"
        );
    }
    let shift = source("x: i64, y: i64", "x >> y");
    let shift_ir = emitted(&shift);
    assert_ne!(
        validate(&shift, &shift_ir.replace("ashr", "shl")).outcome,
        Outcome::EquivalentWithinProfile
    );
    // A short-circuit branch makes `y = 0` a defined source input: an IR
    // that produces a wrong value only there must still be refuted.
    let guarded = source("x: i64, y: i64", "if y != 0 && x / y == 0 { 1 } else { 0 }");
    let ir = pin("define i64 @f(i64 %x, i64 %y) {\n\
         entry:\n\
         %c = icmp ne i64 %y, 0\n\
         br i1 %c, label %div, label %done\n\
         div:\n\
         %v = sdiv i64 %x, %y\n\
         %e = icmp eq i64 %v, 0\n\
         %z = zext i1 %e to i64\n\
         br label %done\n\
         done:\n\
         %r = phi i64 [ 1, %entry ], [ %z, %div ]\n\
         ret i64 %r\n\
         }");
    assert_eq!(validate(&guarded, &ir).outcome, Outcome::Different);
}

#[test]
fn llvm_poison_on_a_source_defined_input_is_undefined_behavior() {
    // `sdiv` by a literal zero is poison on every input, and the source `x`
    // is defined everywhere — a reachable poison, not a value mismatch.
    let total = source("x: i64", "x");
    let ir = function("%v = sdiv i64 %x, 0\nret i64 %v");
    assert_eq!(validate(&total, &ir).outcome, Outcome::UndefinedBehavior);
    // `x sdiv y` poisons on y = 0, which the source's `else` branch defines.
    let guarded = source("x: i64, y: i64", "if y != 0 { x / y } else { x }");
    let ir = pin("define i64 @f(i64 %x, i64 %y) { %v = sdiv i64 %x, %y\nret i64 %v }");
    assert_eq!(validate(&guarded, &ir).outcome, Outcome::UndefinedBehavior);
    // An out-of-range shift is poison the same way: `x shl 64`.
    let ir = function("%v = shl i64 %x, 64\nret i64 %v");
    assert_eq!(validate(&total, &ir).outcome, Outcome::UndefinedBehavior);
    // i64::MIN / -1 is poison in `sdiv`; the source defines nothing there.
    let ir = function("%v = sdiv i64 %x, -1\nret i64 %v");
    let src = source("x: i64", "0 - x");
    assert_eq!(validate(&src, &ir).outcome, Outcome::UndefinedBehavior);
}

#[test]
fn vacuous_and_out_of_profile_division_and_shifts_fail_closed() {
    // A source whose guarded operators bind no input at all is not
    // equivalent — the spec `--bitvec-i64` would reject it outright.
    for body in [
        "x / 0",
        "x % 0",
        "x / (x - x)",
        "x << 64",
        "x >> 64",
        "x << (0 - 1)",
    ] {
        let src = source("x: i64", body);
        assert!(
            tv::validate_pure_i64(&src, &emitted(&src), Options::default()).is_err(),
            "{body}"
        );
    }
    let src = source("x: i64", "x");
    for body in [
        // Flagged arithmetic/div/shift strengthens the poison domain — rejected.
        "%v = add nsw i64 %x, 0\nret i64 %v",
        "%v = add nuw i64 %x, 0\nret i64 %v",
        "%v = sub nsw i64 %x, 0\nret i64 %v",
        "%v = mul nuw i64 %x, 1\nret i64 %v",
        "%v = sdiv exact i64 %x, 2\nret i64 %v",
        "%v = shl nsw i64 %x, 1\nret i64 %v",
        "%v = ashr exact i64 %x, 0\nret i64 %v",
        // Unsigned forms have no source counterpart — still unsupported.
        "%v = udiv i64 %x, 1\nret i64 %v",
        "%v = urem i64 %x, 1\nret i64 %v",
        "%v = lshr i64 %x, 0\nret i64 %v",
        // `i1` division/shifts — legal LLVM, but the profile only encodes
        // i64 semantics; rejecting beats a solver sort mismatch.
        "%c = icmp eq i64 %x, 0\n%v = sdiv i1 %c, %c\n%z = zext i1 %v to i64\nret i64 %z",
        "%c = icmp eq i64 %x, 0\n%v = srem i1 %c, %c\n%z = zext i1 %v to i64\nret i64 %z",
        "%c = icmp eq i64 %x, 0\n%v = shl i1 %c, %c\n%z = zext i1 %v to i64\nret i64 %z",
        "%c = icmp eq i64 %x, 0\n%v = ashr i1 %c, %c\n%z = zext i1 %v to i64\nret i64 %z",
        "%v = freeze i64 %x\nret i64 %v",
        "%p = alloca i64\nstore i64 %x, ptr %p\n%v = load i64, ptr %p\nret i64 %v",
        "%v = call i64 @f(i64 %x)\nret i64 %v",
    ] {
        assert!(
            tv::validate_pure_i64(&src, &function(body), Options::default()).is_err(),
            "{body}"
        );
    }
}

#[test]
fn undef_poison_and_cycles_are_rejected_even_in_dead_blocks_or_branches() {
    let src = source("x: i64", "x");
    for body in [
        "ret i64 undef",
        "ret i64 poison",
        "%v = select i1 true, i64 %x, i64 poison\nret i64 %v",
        "entry: ret i64 %x\ndead: ret i64 undef",
        "entry: ret i64 %x\ndead: %p = icmp eq ptr null, null\nret i64 0",
        "entry: br label %loop\nloop: br label %loop",
        "entry: ret i64 %x\ndead: br label %dead",
    ] {
        assert!(tv::validate_pure_i64(&src, &function(body), Options::default()).is_err());
    }
}

#[test]
fn target_signature_module_and_metadata_mismatches_are_rejected() {
    let src = source("x: i64", "x");
    let ir = function("ret i64 %x");
    let variants = [
        ir.replace(tv::TARGET, "aarch64-unknown-linux-gnu"),
        ir.replace(tv::DATA_LAYOUT, "e-p:64:64"),
        "define i64 @f(i64 %x) { ret i64 %x }".to_owned(),
        ir.replace("@f", "@g"),
        ir.replace("i64 %x", "i32 %x"),
        ir.replace("i64 %x)", "i64 %x, ...)"),
        ir.replace("define i64", "define fastcc i64"),
        ir.replace("define i64", "define noundef i64"),
        ir.replace("i64 %x)", "i64 noundef %x)"),
        ir.replace(") {", ") noreturn {"),
        format!("{ir}\n@g = global i64 0"),
        format!("{ir}\n@a = alias i64 (i64), ptr @f"),
        format!("{ir}\ndeclare i64 @g(i64)"),
        format!("{ir}\ndefine i64 @g(i64 %x) {{ret i64 %x}}"),
        format!("{ir}\nmodule asm \"nop\""),
        ir.replace("ret i64 %x", "ret i64 %x, !annotation !0") + "\n!0 = !{!\"excluded\"}",
    ];
    for input in variants {
        assert!(tv::validate_pure_i64(&src, &input, Options::default()).is_err());
    }
}

#[test]
fn malformed_ir_and_wrong_ssa_dominance_fail_without_echoing_input() {
    let src = source("x: i64", "x");
    for input in ["not IR", "define i64 @f(i64 %x) {ret i64 %missing}", "define i64 @f(i64 %x) {entry: br label %end\ndead: %v = or i64 %x, 0\nbr label %end\nend: ret i64 %v}"] {
        assert!(tv::validate_pure_i64(&src, &pin(input), Options::default()).is_err());
    }
}

#[test]
fn solver_exhaustion_and_invalid_budgets_cannot_be_equivalent() {
    let ir = emitted(CLAMP);
    assert_eq!(
        tv::validate_pure_i64(
            CLAMP,
            &ir,
            Options {
                resource_limit: 1,
                ..Options::default()
            }
        )
        .unwrap()
        .outcome,
        Outcome::Inconclusive
    );
    for options in [
        Options {
            timeout_ms: 0,
            ..Options::default()
        },
        Options {
            resource_limit: 0,
            ..Options::default()
        },
        Options {
            timeout_ms: 30_001,
            ..Options::default()
        },
    ] {
        assert!(tv::validate_pure_i64(CLAMP, &ir, options).is_err());
    }
}

#[test]
fn input_and_instruction_budgets_fail_closed() {
    let src = source("x: i64", "x");
    assert!(tv::validate_pure_i64(
        &format!("//{}\n{src}", "x".repeat(65_536)),
        &function("ret i64 %x"),
        Options::default()
    )
    .is_err());
    assert!(tv::validate_pure_i64(
        &src,
        &format!(";{}\n{}", "x".repeat(65_536), function("ret i64 %x")),
        Options::default()
    )
    .is_err());
    assert!(tv::validate_pure_i64(
        &source("x: i64", &format!("{}x{}", "(".repeat(49), ")".repeat(49))),
        &function("ret i64 %x"),
        Options::default()
    )
    .is_err());
    let params = (0..17)
        .map(|i| format!("x{i}: i64"))
        .collect::<Vec<_>>()
        .join(",");
    assert!(tv::validate_pure_i64(
        &source(&params, "0"),
        &function("ret i64 %x"),
        Options::default()
    )
    .is_err());
    let mut body = String::new();
    for i in 0..1024 {
        body += &format!("%v{i} = or i64 %x, 0\n");
    }
    body += "ret i64 %x";
    assert!(tv::validate_pure_i64(&src, &function(&body), Options::default()).is_err());
}

#[test]
fn block_and_path_expansion_budgets_fail_closed() {
    let src = source("x: i64", "x");
    let mut body = String::from("entry: br label %b0\n");
    for i in 0..64 {
        body += &format!("b{i}: br label %b{}\n", i + 1);
    }
    body += "b64: ret i64 %x";
    assert!(tv::validate_pure_i64(&src, &function(&body), Options::default()).is_err());
    let mut body = String::from("entry: %c = icmp eq i64 %x, 0\nbr label %b0\n");
    for i in 0..9 {
        body += &format!(
            "b{i}: br i1 %c, label %a{i}, label %z{i}\na{i}: br label %b{}\nz{i}: br label %b{}\n",
            i + 1,
            i + 1
        );
    }
    body += "b9: ret i64 %x";
    assert_eq!(
        tv::validate_pure_i64(&src, &function(&body), Options::default())
            .unwrap_err()
            .0,
        "LLVM path expansion limit"
    );
}

#[test]
fn source_literals_comments_and_ir_diagnostics_are_not_echoed() {
    let sentinel = "FAKE_PRIVATE_SOURCE_SENTINEL";
    let src = format!("// {sentinel}\n{}", source("x: i64", "x"));
    let ir = function("ret i64 %x");
    assert!(!format!("{:?}", validate(&src, &ir)).contains(sentinel));
    let error =
        tv::validate_pure_i64(&src, &format!("{sentinel}{ir}"), Options::default()).unwrap_err();
    assert!(!format!("{error:?} {error}").contains(sentinel));
    let literal = "1234567890123456789";
    let src = source("x: i64", literal);
    let ir = function(&format!("ret i64 {literal}"));
    assert!(!format!("{:?}", validate(&src, &ir)).contains(literal));
}

fn native_source(body: &str) -> String {
    format!("atom main() -> i64 requires: true; ensures: true; body: {{ {body} }};")
}

fn native_ir(body: &str) -> String {
    pin(&format!(
        "define i64 @__mumei_user_main() {{ {body} }}\n\
         define i32 @main(i32 %argc, ptr %argv) {{\n\
         %result = call i64 @__mumei_user_main()\n\
         %exit_code = trunc i64 %result to i32\n\
         ret i32 %exit_code\n}}"
    ))
}

fn native_validate(src: &str, ir: &str) -> tv::ValidationReport {
    tv::validate_native_main(src, ir, Options::default()).unwrap()
}

#[test]
fn native_emitter_bodies_and_truncating_wrapper_match_without_rewriting() {
    for body in [
        "0",
        "7",
        "2147483648",
        "4294967296",
        "9223372036854775807",
        "if 1 < 2 { if false { 99 } else { 7 } } else { 42 }",
        "if (1 < 2) && (7 != 8) { 7 } else { 99 }",
        "if (1 > 2) || (7 == 8) { 99 } else { 7 }",
        "7 & 3",
        "7 | 3",
        "7 ^ 3",
        "(7 & 3) | (7 ^ 1)",
        "let y = 7 & 3; y + 1",
        "let a = 7; let b = a | 1; a ^ b",
        "7 + 1; 8",
        "let a = 7; a ^ 1; a",
        "let a = 7; a = a + 1; a",
        "7 - 1; 8",
        "let a = 7; { let a = a + 1; a }; a",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain(
        ["<", "<=", ">", ">=", "==", "!="]
            .into_iter()
            .map(|op| format!("if 7 {op} 8 {{ 7 }} else {{ 8 }}")),
    ) {
        let src = native_source(&body);
        let Item::Atom(atom) = parse_module_checked(&src).unwrap().remove(0) else {
            panic!("atom")
        };
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("native.ll");
        mumei_emit_llvm::binary::compile_atoms_to_binary_ll(
            &[lower_atom_to_hir(&atom)],
            &ModuleEnv::default(),
            &[],
            &path,
        )
        .unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let ir = pin(&raw);
        let report = native_validate(&src, &ir);
        assert_eq!(report.outcome, Outcome::EquivalentWithinProfile, "{body}");
        assert_eq!(report.profile, tv::NATIVE_PROFILE);
        assert_eq!(
            report.source_sha256,
            mumei_core::native_bundle::sha256(src.as_bytes())
        );
        assert_eq!(
            report.llvm_sha256,
            mumei_core::native_bundle::sha256(ir.as_bytes())
        );
        assert_eq!(
            report.input_domain,
            "zero_argument_main_i64_body_not_process_exit_status"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        assert!(tv::validate_pure_i64(&src, &ir, Options::default()).is_err());
    }
}

#[test]
fn native_guarded_division_and_shifts_match_under_the_same_partial_domain() {
    for body in [
        "7 / 1",
        "8 % 3",
        "1 << 3",
        "16 >> 2",
        "if 1 > 0 { 8 / 2 } else { 0 }",
    ] {
        let src = native_source(body);
        let Item::Atom(atom) = parse_module_checked(&src).unwrap().remove(0) else {
            panic!("atom")
        };
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("native.ll");
        mumei_emit_llvm::binary::compile_atoms_to_binary_ll(
            &[lower_atom_to_hir(&atom)],
            &ModuleEnv::default(),
            &[],
            &path,
        )
        .unwrap();
        let ir = pin(&std::fs::read_to_string(&path).unwrap());
        let report = native_validate(&src, &ir);
        assert_eq!(report.outcome, Outcome::EquivalentWithinProfile, "{body}");
        assert_eq!(
            report.input_domain,
            "zero_argument_main_i64_body_where_division_and_shift_defined_not_process_exit_status",
            "{body}"
        );
    }
    // Poison inside the wrapped user-main is `UndefinedBehavior`, not success.
    let src = native_source("7");
    let ir = native_ir("%v = sdiv i64 7, 0\nret i64 %v");
    assert_eq!(
        native_validate(&src, &ir).outcome,
        Outcome::UndefinedBehavior
    );
}

#[test]
fn native_full_i64_body_must_match_even_when_low_32_bits_are_equal() {
    let src = native_source("7");
    for value in ["8", "4294967303", "9223372036854775807"] {
        assert_eq!(
            native_validate(&src, &native_ir(&format!("ret i64 {value}"))).outcome,
            Outcome::Different
        );
    }
    let ir = native_ir("%same = or i64 7, 0\nret i64 %same");
    assert_eq!(
        native_validate(&src, &ir).outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn native_snapshot_profile_is_separate_and_pins_exact_target_and_layout() {
    let src = native_source("7");
    let ir = native_ir("ret i64 7");
    let pc = ir.replace(tv::TARGET, tv::NATIVE_TARGET);
    assert_eq!(native_validate(&src, &pc).target, tv::NATIVE_TARGET);
    for bad in [
        ir.replace(tv::TARGET, "x86_64-pc-windows-msvc"),
        ir.replace(tv::TARGET, "x86_64-unknown-linux-musl"),
        ir.replace(tv::DATA_LAYOUT, "e-i64:64"),
        ir.replace(&format!("target triple = \"{}\"", tv::TARGET), ""),
        ir.replace("__mumei_user_main", "other"),
    ] {
        assert!(tv::validate_native_main(&src, &bad, Options::default()).is_err());
    }
    for bad in [
        source("", "7"),
        source("x: i64", "x"),
        native_source("7 / 0"),
    ] {
        assert!(tv::validate_native_main(&bad, &ir, Options::default()).is_err());
    }
    assert!(tv::validate_native_main(&src, &function("ret i64 7"), Options::default()).is_err());
}

#[test]
fn native_wrapper_must_be_only_a_direct_call_truncation_and_return() {
    let src = native_source("7");
    let ir = native_ir("ret i64 7");
    for (from, to) in [
        ("ret i32 %exit_code", "ret i32 7"),
        ("ret i32 %exit_code", "ret i32 %argc"),
        ("trunc i64 %result", "trunc i64 7"),
        ("trunc i64 %result to i32", "add i32 0, 7"),
        (
            "%result = call",
            "%unused = call i64 @__mumei_user_main()\n%result = call",
        ),
        ("%result = call", "%result = tail call"),
        ("call i64", "call noundef i64"),
        (
            "call i64 @__mumei_user_main()",
            "call i64 @__mumei_user_main() nounwind",
        ),
        (
            "call i64 @__mumei_user_main()",
            "call i64 @__mumei_user_main() [ \"deopt\"(i32 1) ]",
        ),
        ("ptr %argv", "ptr addrspace(1) %argv"),
        ("i32 %argc", "i32 noundef %argc"),
        ("define i32 @main", "define i32 @main_wrong"),
        ("define i32 @main", "define fastcc i32 @main"),
        ("define i32 @main", "define weak i32 @main"),
        (
            "define i64 @__mumei_user_main",
            "define weak i64 @__mumei_user_main",
        ),
        ("ptr %argv)", "ptr %argv, ...)"),
        (
            "%exit_code = trunc",
            "br label %next\nnext: %exit_code = trunc",
        ),
        ("ret i32 %exit_code", "ret i32 %exit_code, !private !0"),
    ] {
        assert_ne!(ir.replace(from, to), ir, "mutation {from}");
        assert!(
            tv::validate_native_main(&src, &ir.replace(from, to), Options::default()).is_err(),
            "{from} -> {to}"
        );
    }
}

#[test]
fn native_module_cannot_hide_extra_functions_globals_metadata_or_instructions() {
    let src = native_source("7");
    let ir = native_ir("ret i64 7");
    for extra in [
        "declare void @external()",
        "define i64 @other() { ret i64 7 }",
        "@global = global i64 7",
        "@alias = alias i64 (), ptr @__mumei_user_main",
        "module asm \"FAKE_PRIVATE_IR_ONLY\"",
        "!private = !{!0}\n!0 = !{i64 7}",
    ] {
        assert!(
            tv::validate_native_main(&src, &format!("{ir}\n{extra}"), Options::default()).is_err()
        );
    }
    for body in [
        "ret i64 poison",
        "ret i64 undef",
        "%p = alloca i64\nret i64 7",
        "%x = add nsw i64 6, 1\nret i64 %x",
        "entry: br label %entry",
        "ret i64 7, !private !0\n}\n!0 = !{i64 7}\ndefine i64 @extra() { ret i64 7",
    ] {
        assert!(tv::validate_native_main(&src, &native_ir(body), Options::default()).is_err());
    }
    for from_to in [
        ("define i64", "define fastcc i64"),
        ("define i64", "define noundef i64"),
        ("@__mumei_user_main() {", "@__mumei_user_main() nounwind {"),
    ] {
        assert!(tv::validate_native_main(
            &src,
            &ir.replace(from_to.0, from_to.1),
            Options::default()
        )
        .is_err());
    }
}

#[test]
fn native_budgets_and_private_parser_diagnostics_fail_closed() {
    let src = native_source("7");
    let ir = native_ir("ret i64 7");
    for options in [
        Options {
            timeout_ms: 0,
            ..Options::default()
        },
        Options {
            timeout_ms: 30_001,
            ..Options::default()
        },
        Options {
            resource_limit: 0,
            ..Options::default()
        },
    ] {
        assert!(tv::validate_native_main(&src, &ir, options).is_err());
    }
    for bad in [
        format!("{ir};{}", "x".repeat(65_536)),
        "[".repeat(49),
        "FAKE_PRIVATE_IR_ONLY".into(),
    ] {
        let error = tv::validate_native_main(&src, &bad, Options::default()).unwrap_err();
        assert!(!format!("{error:?} {error}").contains("FAKE_PRIVATE_IR_ONLY"));
    }
    let report = tv::validate_native_main(
        &src,
        &ir,
        Options {
            resource_limit: 1,
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(report.outcome, Outcome::Inconclusive);
}

#[test]
fn native_empty_operand_bundles_are_not_plain_direct_calls() {
    let src = native_source("7");
    let ir = native_ir("ret i64 7");
    for bundle in ["\"custom\"()", "\"deopt\"()", "\"custom\"(i32 1)"] {
        let changed = ir.replace(
            "call i64 @__mumei_user_main()",
            &format!("call i64 @__mumei_user_main() [ {bundle} ]"),
        );
        assert!(
            tv::validate_native_main(&src, &changed, Options::default()).is_err(),
            "{bundle}"
        );
    }
    let renamed = ir.replace("%result", "%\"call name []\"");
    assert_eq!(
        native_validate(&src, &renamed).outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn scalar_match_is_equivalent_and_refuted() {
    // First-match-order `match` on an i64 scrutinee: literal patterns
    // test `target == n` and a trailing `Variable`/`Wildcard` arm is the
    // catch-all that makes the chain exhaustive — a `Variable` arm also
    // binds the scrutinee value for its body.
    for src in [
        source("x: i64", "match x { 0 => 1, _ => x }"),
        source("x: i64", "match x { 0 => 10, 1 => 20, _ => 0 }"),
        source("x: i64", "match x { k => k + 1 }"),
        source("x: i64", "match x { 0 => 1, v => v }"),
        source("x: i64", "match x { -1 => x, _ => 0 }"),
        source("x: i64", "match x + 1 { 0 => x, _ => 0 }"),
        source("x: i64", "let m = match x { 0 => 0, _ => x }; m * 2"),
        source("x: i64", "match x { 0 => 1, _ => x } + match x { 0 => 0, _ => 1 }"),
        source("x: i64, y: i64", "match x { 0 => x / y, _ => 0 }"),
        "atom f(x: i64) -> bool requires: true; ensures: true; body: { match x { 0 => true, _ => false } };".to_string(),
        source("x: i64", "match x { 0 => { let a = x; a + 1 }, _ => 0 }"),
        // A duplicate literal arm is dead under the accumulated
        // negation; a second catch-all is unreachable; a partial
        // scrutinee keeps its domain obligation; reserved names in a
        // `Variable` pattern bind dead (literals win in the body) or
        // shadow `result` consistently on both sides.
        source("x: i64", "match x { 0 => 1, 0 => 2, _ => 3 }"),
        source("x: i64", "match x { v => v, _ => 0 }"),
        source("x: i64", "match x / 2 { 0 => 1, _ => 0 }"),
        source("x: i64", "match x { true => 1, _ => 0 }"),
        source("x: i64", "match x { result => result }"),
    ] {
        assert_eq!(
            validate(&src, &emitted(&src)).outcome,
            Outcome::EquivalentWithinProfile,
            "{src}"
        );
    }
    // A wrong arm value or test is a real difference, not a signature
    // error.
    let src = source("x: i64", "match x { 0 => 1, _ => x }");
    for ir in [
        pin("define i64 @f(i64 %x) {\nentry:\n  %c = icmp eq i64 %x, 0\n  br i1 %c, label %a, label %b\na:\n  ret i64 5\nb:\n  ret i64 %x\n}"),
        pin("define i64 @f(i64 %x) {\nentry:\n  %c = icmp eq i64 %x, 1\n  br i1 %c, label %a, label %b\na:\n  ret i64 1\nb:\n  ret i64 %x\n}"),
        pin("define i64 @f(i64 %x) { ret i64 %x }"),
    ] {
        assert_eq!(validate(&src, &ir).outcome, Outcome::Different, "{ir}");
    }
    // An `unreachable` that can execute on a defined input is LLVM UB —
    // here an IR match that drops the catch-all leaves x = 5 without a
    // return.
    assert_eq!(
        validate(
            &src,
            &pin("define i64 @f(i64 %x) {\nentry:\n  %c = icmp eq i64 %x, 0\n  br i1 %c, label %a, label %u\na:\n  ret i64 1\nu:\n  unreachable\n}")
        )
        .outcome,
        Outcome::UndefinedBehavior
    );
    // The same first-match chain spelled as `if` compares equal.
    let same = emitted(&source("x: i64", "if x == 0 { 1 } else { x }"));
    assert_eq!(
        validate(&src, &same).outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn all_unreachable_function_reports_ub_not_a_missing_return() {
    // With no `ret` edge at all, every path ends in `unreachable` — that
    // is UB on every defined input, not a missing-return error.
    let src = source("x: i64", "x");
    for ir in [
        pin("define i64 @f(i64 %x) {\nentry:\n  unreachable\n}"),
        pin("define i64 @f(i64 %x) {\nentry:\n  %c = icmp eq i64 %x, 0\n  br i1 %c, label %u, label %v\nu:\n  unreachable\nv:\n  unreachable\n}"),
    ] {
        assert_eq!(validate(&src, &ir).outcome, Outcome::UndefinedBehavior, "{ir}");
    }
}

#[test]
fn scalar_match_stays_bounded() {
    // Non-admitted shapes are static errors, not silently weakened.
    for src in [
        // Literal-only coverage cannot be exhaustive on i64 — a
        // catch-all arm is required.
        source("x: i64", "match x { 0 => 1 }"),
        source("x: i64", "match x { 0 => 1, 1 => 2 }"),
        // Arm guards stay outside the profile.
        source("x: i64", "match x { k if k > 0 => k, _ => 0 }"),
        // A non-i64 scrutinee stays outside the profile.
        "atom f(b: bool) -> i64 requires: true; ensures: true; body: { match b { _ => 1 } };"
            .to_string(),
        // Variant patterns stay outside the profile.
        source("x: i64", "match x { Some(k) => k, _ => 0 }"),
        // Arms must agree on one kind.
        source("x: i64", "match x { 0 => true, _ => 0 }"),
    ] {
        assert!(tv::validate_pure_i64(&src, &function("ret i64 0"), Options::default()).is_err());
    }
}
