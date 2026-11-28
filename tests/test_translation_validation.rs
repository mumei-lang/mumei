use mumei_core::hir::lower_atom_to_hir;
use mumei_core::parser::{parse_module_checked, Item};
use mumei_core::semantic_contract::{
    RefinedAdmission, PURE_I64_REFINED_PROFILE, PURE_I64_WRAPPING_PROFILE,
};
use mumei_core::verification::ModuleEnv;
use mumei_emit_llvm::translation_validation::{self as tv, Options, Outcome};
use std::collections::BTreeMap;

const CLAMP: &str = include_str!("fixtures/semantic_contract_clamp.mm");
const TRANSFER_IR: &str = include_str!("fixtures/translation_validation_transfer.ll");
const TRANSFER_SOURCE: &str = r#"
struct Transfer {
    ok: bool,
    src: i64,
    dst: i64,
    house: i64,
}

atom transfer(src: i64, dst: i64, house: i64, dst_exists: bool, distinct_open: bool, amount: i64) -> Transfer
    requires: src >= 0;
    ensures: result.ok => amount > 0;
    body: {
        if amount <= 0 { Transfer { ok: false, src: src, dst: dst, house: house } }
        else { if !dst_exists || !distinct_open { Transfer { ok: false, src: src, dst: dst, house: house } }
        else { if !(src >= amount + 25) { Transfer { ok: false, src: src, dst: dst, house: house } }
        else { Transfer { ok: true, src: src - amount - 25, dst: dst + amount, house: house + 25 } } } }
    }

atom main() -> i64
    requires: true;
    ensures: result == 0;
    body: { 0 };
"#;

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

/// The widest caller budget: guarded-division equivalences sit near the
/// default 5s/1M limit and report `Inconclusive` on loaded runners,
/// though the contract accepts budgets up to 30s. Negative checks below
/// keep the default budget so they stay honest about timing out.
fn validate_wide(source: &str, ir: &str) -> tv::ValidationReport {
    tv::validate_pure_i64(
        source,
        ir,
        Options {
            timeout_ms: 30_000,
            resource_limit: 4_000_000,
        },
    )
    .unwrap()
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
    // No loop is admitted, so no bound is claimed.
    assert_eq!(report.unroll_bound, None);
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
        // `while` is admitted only mid-block with a boolean guard and
        // no nested loop: a non-boolean guard, a nested loop and a
        // loop in value position all stay unsupported.
        "while x invariant true { x }; x",
        "while x > 0 invariant true { while x > 0 invariant true { x }; x }; x",
        "while x > 0 invariant true { x }",
        // Sequences admit `let`s, scalar reassignments and pure
        // expression statements: an unbound assignment and a trailing
        // `let` without a value still fail.
        "y = 1; x",
        "let y = x;",
        // Rebinding cannot escape a nested block, and a name not bound
        // on the linear path cannot be assigned inside a branch.
        "let a = x; { a = 1; a }; a",
        "if x > 0 { y = 1; y } else { 0 }",
        // Branch writes stay closed in every non-statement position:
        // operands and `let` initializers cannot merge an escaping
        // write, and a name not bound on the linear path cannot be
        // written inside a match arm either.
        "x + (if x > 0 { x = 1; x } else { x })",
        "let a = if x > 0 { x = 1; x } else { x }; a",
        "match x { 0 => { y = 1; y }, _ => { x } }",
        // Reassignment keeps the binding's kind, inside a branch too.
        "let a = x; a = x > 0; a",
        "let b = x > 0; if b { x = b; x } else { x }",
        "if x > 0 { x = x > 1; x } else { x }; x",
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
    for params in ["x: u64", "x: [i64]", "x: f64"] {
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
    let req = "atom f(b: bool, x: i64) -> i64 requires: b; ensures: true; body: { if b { x } else { 0 } };".to_string();
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
    // Decomposed edge obligations are small enough for this formerly
    // budget-hard nonlinear case; solver exhaustion still fails closed in
    // `solver_exhaustion_and_invalid_budgets_cannot_be_equivalent`.
    let guarded = source("x: i64", "if x + 1 > 0 { x * x } else { 0 - x }");
    assert_eq!(
        validate(&guarded, &emitted(&guarded)).outcome,
        Outcome::EquivalentWithinProfile
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
fn conditional_path_reassignment_is_equivalent_and_refuted() {
    for (params, body) in [
        // A bare assignment yields its assigned value: `x = 1` returns 1.
        ("x: i64", "x = 1"),
        ("x: i64", "x = x + 1"),
        // Both branches rebind a parameter: the value merges under the
        // branch condition like the emitter's per-name phi.
        ("x: i64", "if x > 0 { x = 1; x } else { x = 2; x }"),
        ("x: i64", "if x > 0 { x = 1; x } else { x = 2; x }; x"),
        // A name written on one side keeps its prior value on the
        // other — the merge carries the untouched binding through.
        ("x: i64", "if x > 0 { x = 1; x } else { x }; x"),
        ("x: i64", "let a = x; if x > 0 { a = 1; a } else { a }; a"),
        // Several linear-path names merge independently: a reassigned
        // branch-local `let` also merges for the rest of its block.
        (
            "x: i64",
            "let y = x; if x > 0 { y = 9; x = 1; x } else { x }; x + y",
        ),
        // Sequential conditionals see the earlier merges.
        (
            "x: i64",
            "if x > 0 { x = 1; x } else { x }; if x > 2 { x = 3; x } else { x }; x",
        ),
        // A nested conditional merges inside the enclosing branch.
        (
            "x: i64",
            "if x > 0 { if x > 1 { x = 5; x } else { x }; x } else { x }; x",
        ),
        // A branch-local `let` shadows nothing and stays lexical; the
        // merged write to `x` still escapes its branch.
        (
            "x: i64",
            "if x > 0 { let a = x; a = a + 1; x = a; x } else { x }; x",
        ),
        // A `let` bound in the same block shadows its name for escape:
        // writes to it merge only inside that block's scope.
        ("x: i64", "let x = 7; if x > 0 { x = 1; x } else { x }; x"),
        // Boolean bindings merge the same way.
        (
            "x: i64",
            "let b = x > 0; if x > 1 { b = false; b } else { b }; if b { x } else { 0 }",
        ),
    ] {
        let src = source(params, body);
        assert_eq!(
            validate(&src, &emitted(&src)).outcome,
            Outcome::EquivalentWithinProfile,
            "{body}"
        );
    }
    // A partial operation inside a branch keeps its definedness
    // obligation under the refined guard: the shift and division bind
    // only where their guards let them run — matching the guarded
    // emitter path.
    for (params, body) in [
        (
            "x: i64, y: i64",
            "if y < 64 { x = x << y; x } else { x }; x",
        ),
        ("x: i64, y: i64", "if y != 0 { x = x / 4; x } else { x }; x"),
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
    // A variable divisor under a guard also becomes a small independent
    // edge obligation once decomposed; the shared division domain still
    // binds both sides.
    let src = source("x: i64, y: i64", "if y != 0 { x = x / y; x } else { x }; x");
    assert_eq!(
        validate(&src, &emitted(&src)).outcome,
        Outcome::EquivalentWithinProfile
    );
    // The merged value, not the pre-branch value, reaches the tail.
    let src = source("x: i64", "if x > 0 { x = 1; x } else { x }; x");
    for wrong_ir in [
        // A return of the untouched parameter ignores the merge.
        pin("define i64 @f(i64 %x) { ret i64 %x }"),
        // Phi operands swapped across the condition.
        pin("define i64 @f(i64 %x) {\nentry:\n  %c = icmp sgt i64 %x, 0\n  br i1 %c, label %t, label %e\nt:\n  br label %m\ne:\n  br label %m\nm:\n  %p = phi i64 [ %x, %t ], [ 1, %e ]\n  ret i64 %p\n}"),
        // A constant instead of the conditional write.
        pin("define i64 @f(i64 %x) { ret i64 1 }"),
    ] {
        assert_eq!(
            validate(&src, &wrong_ir).outcome,
            Outcome::Different,
            "{wrong_ir}"
        );
    }
    // Merging the wrong literal is different too.
    let wrong = emitted(&source("x: i64", "if x > 0 { x = 2; x } else { x }; x"));
    assert_eq!(validate(&src, &wrong).outcome, Outcome::Different);
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
        let report = validate_wide(&src, &emitted(&src));
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
    assert_eq!(
        validate_wide(&src, &wrong_const).outcome,
        Outcome::Different
    );
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
    assert_eq!(validate_wide(&guarded, &ir).outcome, Outcome::Different);
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
    assert_eq!(
        validate_wide(&guarded, &ir).outcome,
        Outcome::UndefinedBehavior
    );
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
        // `i1` division/shifts — legal LLVM, but the profile only encodes
        // i64 semantics; rejecting beats a solver sort mismatch.
        "%c = icmp eq i64 %x, 0\n%v = sdiv i1 %c, %c\n%z = zext i1 %v to i64\nret i64 %z",
        "%c = icmp eq i64 %x, 0\n%v = srem i1 %c, %c\n%z = zext i1 %v to i64\nret i64 %z",
        "%c = icmp eq i64 %x, 0\n%v = shl i1 %c, %c\n%z = zext i1 %v to i64\nret i64 %z",
        "%c = icmp eq i64 %x, 0\n%v = ashr i1 %c, %c\n%z = zext i1 %v to i64\nret i64 %z",
        "%v = freeze i64 %x\nret i64 %v",
        "%p = alloca i64\nstore i64 %x, ptr %p\n%v = load i64, ptr %p\nret i64 %v",
    ] {
        assert!(
            tv::validate_pure_i64(&src, &function(body), Options::default()).is_err(),
            "{body}"
        );
    }
    // Unsigned forms are encoded (composite v10 admits `u64`) — they are
    // not rejected, but `udiv x,2`, `urem x,1` (0) and `lshr x,1` diverge
    // from the `x` source and must report `Different`.
    for body in [
        "%v = udiv i64 %x, 2\nret i64 %v",
        "%v = urem i64 %x, 1\nret i64 %v",
        "%v = lshr i64 %x, 1\nret i64 %v",
    ] {
        assert_eq!(
            validate(&src, &function(body)).outcome,
            Outcome::Different,
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
    // eleven chained branch diamonds explore up to 2048 states, so the
    // path-expansion budget still fails closed even at the raised cap.
    for i in 0..11 {
        body += &format!(
            "b{i}: br i1 %c, label %a{i}, label %z{i}\na{i}: br label %b{}\nz{i}: br label %b{}\n",
            i + 1,
            i + 1
        );
    }
    body += "b11: ret i64 %x";
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
fn match_arm_guards_are_equivalent_and_refuted() {
    // An arm hits when its pattern matches and its Boolean guard holds:
    // a guarded `Variable` arm, a guarded literal falling through to the
    // same literal unguarded, a guarded wildcard mid-chain, a bool
    // parameter guard and a partial operator inside the guard all follow
    // the emitter's `pattern ∧ guard` try-chain.
    for src in [
        source("x: i64", "match x { k if k > 0 => k, _ => 0 }"),
        source("x: i64", "match x { _ if x > 0 => 1, _ => 0 }"),
        source(
            "x: i64, y: i64",
            "match x { 0 if y > 0 => 1, 0 => 2, _ => 3 }",
        ),
        source(
            "x: i64, y: i64",
            "match x { k if k > y => k, 5 if y > 0 => 5, _ => 0 }",
        ),
        source("x: i64, b: bool", "match x { k if b => k, _ => 0 }"),
        source("x: i64", "match x { k if k / 4 > 0 => k, _ => 0 }"),
        // A guarded arm after a catch-all is dead and binds nothing.
        source("x: i64", "match x { _ => 0, k if k > 0 => k, _ => 1 }"),
    ] {
        assert_eq!(
            validate(&src, &emitted(&src)).outcome,
            Outcome::EquivalentWithinProfile,
            "{src}"
        );
    }
    // A guarded chain spelled as nested `if`s compares equal.
    let guarded = source(
        "x: i64, y: i64",
        "match x { 0 if y > 0 => 1, 0 => 2, _ => 3 }",
    );
    let as_ifs = emitted(&source(
        "x: i64, y: i64",
        "if x == 0 { if y > 0 { 1 } else { 2 } } else { 3 }",
    ));
    assert_eq!(
        validate(&guarded, &as_ifs).outcome,
        Outcome::EquivalentWithinProfile
    );
    // Dropping or weakening the guard is a real value difference:
    // at `x = -1` the guarded source yields 42 where the unguarded IR
    // returns -1, and `k >= 0` differs from `k > 0` at `x = 0`.
    let src = source("x: i64", "match x { k if k > 0 => k, _ => 42 }");
    for other in [
        emitted(&source("x: i64", "x")),
        emitted(&source("x: i64", "match x { k if k >= 0 => k, _ => 42 }")),
        emitted(&source("x: i64", "if x > 0 { x } else { 1 }")),
    ] {
        assert_eq!(validate(&src, &other).outcome, Outcome::Different);
    }
    // A guarded catch-all may fall through — the trailing arm must be
    // unguarded, and a non-Boolean guard is a kind error, both static.
    for src in [
        source("x: i64", "match x { 0 => 1, _ if x > 0 => 2 }"),
        source("x: i64", "match x { k if k + 1 => k, _ => 0 }"),
    ] {
        assert!(tv::validate_pure_i64(&src, &function("ret i64 0"), Options::default()).is_err());
    }
}

#[test]
fn match_arm_reassignment_is_equivalent_and_refuted() {
    // Arm bodies may rebind linear-path names: each write merges under
    // the arm-local hit (earlier misses ∧ pattern ∧ guard), matching
    // the emitter's match.merge per-name phi — a name written on one
    // arm keeps its prior value on the others.
    for src in [
        // Writes on every arm merge under each arm's own hit.
        source(
            "x: i64",
            "let r = 0; match x { 0 => { r = 1; r }, _ => { r = 2; r } }; r",
        ),
        // A name written on one arm keeps its prior value elsewhere.
        source(
            "x: i64",
            "let r = x; match x { 0 => { r = 0; r }, _ => r }; r",
        ),
        // Sequential matches see the earlier merges.
        source(
            "x: i64",
            "let r = 0; match x { 0 => { r = 1; r }, _ => { r = 2; r } }; match r { 1 => x, _ => x + r }",
        ),
        // A guarded arm's writes merge under `pattern ∧ guard`.
        source(
            "x: i64, y: i64",
            "let r = 0; match x { k if y > 0 => { r = k; r }, _ => { r = 1; r } }; r",
        ),
        // A `Variable` pattern binding shadows nothing linear: writes
        // to an outer name still escape.
        source(
            "x: i64",
            "let r = 0; match x { v => { r = v; r } }; r",
        ),
        // A match nested in a branch merges inside the enclosing
        // branch.
        source(
            "x: i64",
            "let r = 0; if x > 0 { match x { 0 => { r = 1; r }, _ => { r = 2; r } }; r } else { r }; r",
        ),
        // An arm-local `let` shadows its name: writes to it stay in
        // the arm, while the outer `r` write still escapes.
        source(
            "x: i64",
            "let r = 0; match x { 0 => { let t = x; t = t + 1; r = t; r }, _ => { r = 9; r } }; r",
        ),
        // Boolean bindings merge the same way.
        source(
            "x: i64",
            "let b = false; match x { 0 => { b = true; b }, _ => { b = false; b } }; if b { x } else { 0 }",
        ),
    ] {
        assert_eq!(
            validate(&src, &emitted(&src)).outcome,
            Outcome::EquivalentWithinProfile,
            "{src}"
        );
    }
    // A wrong merged value is a real difference: at x = 0 the source
    // yields 1, not 2.
    let src = source(
        "x: i64",
        "let r = 0; match x { 0 => { r = 1; r }, _ => { r = 2; r } }; r",
    );
    for other in [
        emitted(&source("x: i64", "2")),
        emitted(&source("x: i64", "x")),
        emitted(&source("x: i64", "if x == 0 { 2 } else { 1 }")),
    ] {
        assert_eq!(validate(&src, &other).outcome, Outcome::Different);
    }
    // The same first-match order spelled as `if` branches compares
    // equal — including the writes.
    let as_ifs = emitted(&source(
        "x: i64",
        "let r = 0; if x == 0 { r = 1; r } else { r = 2; r }; r",
    ));
    assert_eq!(
        validate(&src, &as_ifs).outcome,
        Outcome::EquivalentWithinProfile
    );
    // A `Variable` pattern binding that shadows an assignable name
    // keeps that name arm-local: writing it is outside the profile.
    let shadowed = source("x: i64", "match x { x => { x = 1; x } }");
    assert!(tv::validate_pure_i64(&shadowed, &function("ret i64 0"), Options::default()).is_err());
    // A match in operand position keeps its arm writes lexical: a `let`
    // initializer whose arm rebinds an outer name is unsupported.
    let operand = source(
        "x: i64",
        "let r = 0; let v = match x { 0 => { r = 1; r }, _ => { r = 2; r } }; v + r",
    );
    assert!(tv::validate_pure_i64(&operand, &function("ret i64 0"), Options::default()).is_err());
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
        // A guarded catch-all arm may fall through, so the trailing arm
        // must be unguarded; a non-Boolean guard is a kind error.
        source("x: i64", "match x { 0 => 1, _ if x > 0 => 2 }"),
        source("x: i64", "match x { k if k + 1 => k, _ => 0 }"),
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

#[test]
fn subrange_integer_parameters_are_equivalent_and_refuted() {
    // Declared subrange integer types lower to an i64 formal like
    // `bool`, and the declared signature bounds the checked input to
    // the type's range through `param_domain`.
    for src in [
        source("x: i8", "x"),
        source("x: i8", "x * 2"),
        source("x: u8", "x + 1"),
        source("x: i16", "if x < 0 { 0 - x } else { x }"),
        source("x: i32", "x * x"),
        source("x: u16", "x & 255"),
        source("x: u32", "x >> 1"),
        source("x: u8", "x / 4"),
        source("x: i8, y: i64", "x + y"),
        source("x: i32, b: bool", "if b { x } else { 0 }"),
        source("x: i32", "if x > 100 { x } else { 0 }"),
    ] {
        assert_eq!(
            validate(&src, &emitted(&src)).outcome,
            Outcome::EquivalentWithinProfile,
            "{src}"
        );
    }
    // A signature-bounded domain reports as such, not as a bool domain.
    let src = source("x: i8", "x");
    assert_eq!(
        validate(&src, &emitted(&src)).input_domain,
        "signed_i64_inputs_with_signature_bounded_param_domains_not_restricted_by_requires"
    );
    // The declared type bounds the checked inputs: `x == 200` is always
    // false inside `i8` and `x == 300` inside `u8`, so these bodies are
    // equivalent to `ret 0` only because `param_domain` rules out the
    // inputs that would differ.
    for src in [
        source("x: i8", "if x == 200 { 1 } else { 0 }"),
        source("x: u8", "if x == 300 { 1 } else { 0 }"),
        source("x: i32", "if x == 3000000000 { 1 } else { 0 }"),
    ] {
        assert_eq!(
            validate(&src, &function("ret i64 0")).outcome,
            Outcome::EquivalentWithinProfile,
            "{src}"
        );
    }
    // A body that differs inside the domain is still `Different`, and
    // the same out-of-range literal against a plain `i64` parameter
    // stays a real difference at `x = 200`.
    assert_eq!(
        validate(&source("x: i8", "x + 1"), &function("ret i64 %x")).outcome,
        Outcome::Different
    );
    assert_eq!(
        validate(
            &source("x: i64", "if x == 200 { 1 } else { 0 }"),
            &function("ret i64 0")
        )
        .outcome,
        Outcome::Different
    );
    // Full-width unsigned, target-width and unknown types stay outside
    // the profile — all static signature errors.
    for src in [
        "atom f(x: u64) -> i64 requires: true; ensures: true; body: { x };",
        "atom f(x: usize) -> i64 requires: true; ensures: true; body: { x };",
        "atom f(x: isize) -> i64 requires: true; ensures: true; body: { x };",
        "atom f(x: i128) -> i64 requires: true; ensures: true; body: { x };",
        "atom f(x: i64) -> u64 requires: true; ensures: true; body: { x };",
        "atom f(x: i64) -> usize requires: true; ensures: true; body: { x };",
        "atom f(x: i64) -> isize requires: true; ensures: true; body: { x };",
        "atom f(x: i64) -> i128 requires: true; ensures: true; body: { x };",
    ] {
        assert!(tv::validate_pure_i64(src, &function("ret i64 0"), Options::default()).is_err());
    }
    // A declared subrange integer result lowers to an i64 return the
    // same way: the emitter emits no truncation, so `-> i32` compares
    // the body's full i64 value against the emitted module.
    for src in [
        "atom f(x: i64) -> i8 requires: true; ensures: true; body: { x };",
        "atom f(x: i64) -> i32 requires: true; ensures: true; body: { x + 1 };",
        "atom f(x: i32) -> u8 requires: true; ensures: true; body: { x & 255 };",
        "atom f(x: u8) -> i16 requires: true; ensures: true; body: { x * 2 };",
        "atom f(x: i8, y: i8) -> i32 requires: true; ensures: true; body: { x + y };",
    ] {
        assert_eq!(
            validate(src, &emitted(src)).outcome,
            Outcome::EquivalentWithinProfile,
            "{src}"
        );
    }
    // An IR returning a different value inside the result type's domain
    // is still `Different`, and a `-> i32` body producing an i64 kind
    // keeps the same encoder path.
    let src = "atom f(x: i64) -> i32 requires: true; ensures: true; body: { x + 1 };";
    assert_eq!(
        validate(src, &function("ret i64 %x")).outcome,
        Outcome::Different
    );
}

const CALLING: &str = "atom g(x: i64) -> i64 requires: true; ensures: true; body: { x }; atom f(y: i64) -> i64 requires: true; ensures: true; body: { g(y) + g(y + 1) };";

fn calling_ir(callee: &str) -> String {
    pin(&format!(
        "define i64 @g(i64 %x) {{ ret i64 %x }}
         define i64 @h(i64 %x) {{ %d = sub i64 0, %x ret i64 %d }}
         define i64 @f(i64 %y) {{
           %a = call i64 @{callee}(i64 %y)
           %b = add i64 %y, 1
           %c = call i64 @{callee}(i64 %b)
           %d = add i64 %a, %c
           ret i64 %d
         }}"
    ))
}

#[test]
fn direct_calls_to_in_module_functions_are_equivalent_and_refuted() {
    // Both sides model `g`/`h` as the same uninterpreted function of
    // their arguments, so identical call sites prove equal and a
    // different callee refutes.
    let report = validate(CALLING, &calling_ir("g"));
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);

    let swapped = validate(CALLING, &calling_ir("h"));
    assert_eq!(swapped.outcome, Outcome::Different);

    // A quoted SSA name containing a bracket is not an operand
    // bundle: `%"a [0]"` stays a plain call result.
    let quoted = pin("define i64 @g(i64 %x) { ret i64 %x }
         define i64 @f(i64 %y) { %\"a [0]\" = call i64 @g(i64 %y) %b = add i64 %y, 1 %c = call i64 @g(i64 %b) %d = add i64 %\"a [0]\", %c ret i64 %d }");
    assert_eq!(
        validate(CALLING, &quoted).outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn calls_require_plain_defined_i64_in_module_targets() {
    // A declaration-only callee carries no body and stays outside the
    // profile, and so do mismatched signatures, tail calls and
    // attribute-carrying call sites.
    let declared = pin("declare i64 @g(i64)
         define i64 @f(i64 %y) { %a = call i64 @g(i64 %y) ret i64 %a }");
    assert!(tv::validate_pure_i64(CALLING, &declared, Options::default()).is_err());

    let narrow = pin("define i32 @g(i32 %x) { ret i32 %x }
         define i64 @f(i64 %y) { %a = call i32 @g(i32 %y) ret i64 %a }");
    assert!(tv::validate_pure_i64(CALLING, &narrow, Options::default()).is_err());

    // Call-site attributes — return, parameter and function-index —
    // stay outside the profile even on an all-i64 signature.
    let param_attr = pin("define i64 @g(i64 %x) { ret i64 %x }
         define i64 @h(i64 %x) { %d = sub i64 0, %x ret i64 %d }
         define i64 @f(i64 %y) { %a = call i64 @g(i64 noundef %y) %b = add i64 %y, 1 %c = call i64 @g(i64 %b) %d = add i64 %a, %c ret i64 %d }");
    assert!(tv::validate_pure_i64(CALLING, &param_attr, Options::default()).is_err());
    let tail = pin("define i64 @g(i64 %x) { ret i64 %x }
         define i64 @f(i64 %y) { %a = tail call i64 @g(i64 %y) %b = add i64 %y, 1 %c = tail call i64 @g(i64 %b) %d = add i64 %a, %c ret i64 %d }");
    assert!(tv::validate_pure_i64(CALLING, &tail, Options::default()).is_err());
    // A call whose operand count exceeds args + 1 — an operand bundle
    // in real IR — is rejected even when its tail looks plain; a
    // vararg signature and an arity-mismatched call site are too.
    let vararg = pin("define i64 @g(i64 %x, ...) { ret i64 %x }
         define i64 @f(i64 %y) { %a = call i64 (i64, ...) @g(i64 %y, i64 %y) %b = add i64 %y, 1 %c = call i64 (i64, ...) @g(i64 %b, i64 %y) %d = add i64 %a, %c ret i64 %d }");
    assert!(tv::validate_pure_i64(CALLING, &vararg, Options::default()).is_err());
    let extra_operand = pin("define i64 @g(i64 %x) { ret i64 %x }
         define i64 @f(i64 %y) { %a = call i64 @g(i64 %y, i64 %y) ret i64 %a }");
    assert!(tv::validate_pure_i64(CALLING, &extra_operand, Options::default()).is_err());
    // An empty operand bundle adds no operand — the printed form's
    // bracket is what rejects it.
    let bundle = pin("define i64 @g(i64 %x) { ret i64 %x }
         define i64 @f(i64 %y) { %a = call i64 @g(i64 %y) [ \"deopt\"() ] %b = add i64 %y, 1 %c = call i64 @g(i64 %b) [ \"deopt\"() ] %d = add i64 %a, %c ret i64 %d }");
    assert!(tv::validate_pure_i64(CALLING, &bundle, Options::default()).is_err());
}

#[test]
fn module_validation_checks_each_atom_against_the_same_named_member() {
    // The callee's body is its own obligation: with both members right
    // every entry is equivalent — including `h`, an extra member that is
    // screened but never compared.
    let report =
        tv::validate_pure_i64_module(CALLING, &calling_ir("g"), Options::default()).unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    assert_eq!(
        report
            .functions
            .iter()
            .map(|function| (function.function_name.as_str(), function.outcome))
            .collect::<Vec<_>>(),
        vec![
            ("g", Outcome::EquivalentWithinProfile),
            ("f", Outcome::EquivalentWithinProfile)
        ]
    );

    // `f` calling `@h` refutes only f's obligation.
    let report =
        tv::validate_pure_i64_module(CALLING, &calling_ir("h"), Options::default()).unwrap();
    assert_eq!(report.outcome, Outcome::Different);
    assert_eq!(
        report.functions[0].outcome,
        Outcome::EquivalentWithinProfile
    );
    assert_eq!(report.functions[1].outcome, Outcome::Different);

    // A wrong callee body fails only its own entry: the caller still
    // applies the same uninterpreted `g`, so the modular comparison
    // keeps `f` equivalent — compositionality is what makes the
    // per-function obligations independent.
    let wrong_callee = pin("define i64 @g(i64 %x) { %d = add i64 %x, 1 ret i64 %d }
         define i64 @f(i64 %y) {
           %a = call i64 @g(i64 %y)
           %b = add i64 %y, 1
           %c = call i64 @g(i64 %b)
           %d = add i64 %a, %c
           ret i64 %d
         }");
    let report = tv::validate_pure_i64_module(CALLING, &wrong_callee, Options::default()).unwrap();
    assert_eq!(report.outcome, Outcome::Different);
    assert_eq!(report.functions[0].outcome, Outcome::Different);
    assert_eq!(
        report.functions[1].outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn module_validation_requires_every_source_atom_as_a_member() {
    // `f` absent: its call-site comparison cannot even be formed.
    let missing = pin("define i64 @g(i64 %x) { ret i64 %x }");
    assert!(tv::validate_pure_i64_module(CALLING, &missing, Options::default()).is_err());
    // A single-atom module reports exactly one entry, matching the
    // single-function route's verdict.
    let single = source("x: i64", "x + 1");
    let ir = function("%d = add i64 %x, 1 ret i64 %d");
    let report = tv::validate_pure_i64_module(&single, &ir, Options::default()).unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    assert_eq!(report.functions.len(), 1);
    assert_eq!(report.functions[0].function_name, "f");
}

#[test]
fn emitted_multi_atom_module_validates_per_atom() {
    // End to end: the real emitter's whole-module output — `f` calling
    // `@g` — checks per atom, so a sabotaged callee body fails only its
    // own entry.
    let items = parse_module_checked(CALLING).unwrap();
    let hirs: Vec<_> = items
        .iter()
        .map(|item| {
            let Item::Atom(atom) = item else {
                panic!("atom")
            };
            lower_atom_to_hir(atom)
        })
        .collect();
    let context = mumei_emit_llvm::LlvmContext::create();
    let module = context.create_module("calling");
    mumei_emit_llvm::codegen::compile_atoms_into_module(
        &context,
        &module,
        &hirs,
        &ModuleEnv::default(),
        &[],
    )
    .unwrap();
    let emitted_ir = pin(&module.print_to_string().to_string());
    let report = tv::validate_pure_i64_module(CALLING, &emitted_ir, Options::default()).unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    assert_eq!(report.functions.len(), 2);
    assert!(report
        .functions
        .iter()
        .all(|function| function.outcome == Outcome::EquivalentWithinProfile));
}

#[test]
fn module_validation_resolves_forward_calls_to_later_atoms() {
    // Declaration order is irrelevant: `f` (first) calling `g` (last)
    // resolves the same as the backward direction.
    let forward = "atom f(y: i64) -> i64 requires: true; ensures: true; body: { g(y) + g(y + 1) }; atom g(x: i64) -> i64 requires: true; ensures: true; body: { x };";
    let report =
        tv::validate_pure_i64_module(forward, &calling_ir("g"), Options::default()).unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    // A non-last atom calling an undeclared target still fails closed —
    // every atom's contract is extracted, not only the checked tail's.
    let undeclared = "atom g(x: i64) -> i64 requires: true; ensures: true; body: { missing(x) }; atom f(y: i64) -> i64 requires: true; ensures: true; body: { y };";
    assert!(
        tv::validate_pure_i64_module(undeclared, &calling_ir("g"), Options::default()).is_err()
    );
}

const NATIVE_CALLING: &str = "atom g(x: i64) -> i64 requires: true; ensures: true; body: { x + 1 }; atom main() -> i64 requires: true; ensures: true; body: { g(40) + 2 };";

fn native_module_ir(g_body: &str) -> String {
    pin(&format!(
        "define i64 @g(i64 %x) {{ {g_body} }}\n\
         define i64 @__mumei_user_main() {{ %a = call i64 @g(i64 40) %r = add i64 %a, 2 ret i64 %r }}\n\
         define i32 @main(i32 %argc, ptr %argv) {{\n\
         %result = call i64 @__mumei_user_main()\n\
         %exit_code = trunc i64 %result to i32\n\
         ret i32 %exit_code\n}}"
    ))
}

#[test]
fn native_module_bool_result_calls_carry_the_zero_one_convention() {
    // A `-> Flag` member forwarding a bool call result materializes
    // `ite(v != 0, 1, 0)` on the source side while the IR `ret`s the
    // raw uninterpreted value: without the convention bound the UF
    // result `v = 2` is a spurious difference. The bound lives on the
    // shared application term, so the same call compares equal on both
    // sides.
    let module = "type Flag = bool where v == true;\n\
         atom f(x: Flag) -> Flag requires: true; ensures: true; body: { x };\n\
         atom g(x: Flag) -> Flag requires: true; ensures: true; body: { f(x) };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };";
    let ir = pin("define i64 @f(i64 %x) { ret i64 %x }\n\
         define i64 @g(i64 %x) { %r = call i64 @f(i64 %x) ret i64 %r }\n\
         define i64 @__mumei_user_main() { ret i64 0 }\n\
         define i32 @main(i32 %argc, ptr %argv) {\n\
         %result = call i64 @__mumei_user_main()\n\
         %exit_code = trunc i64 %result to i32\n\
         ret i32 %exit_code\n}");
    let report =
        tv::validate_native_module(module, &ir, Options::default(), RefinedAdmission::V2).unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    // The bound is the convention, not an unconditional pass: a member
    // returning a literal outside {0,1} for a bool result still
    // differs from the normalized source.
    let bad = ir.replace(
        "%r = call i64 @f(i64 %x) ret i64 %r",
        "%r = call i64 @f(i64 %x) ret i64 5",
    );
    let report =
        tv::validate_native_module(module, &bad, Options::default(), RefinedAdmission::V2).unwrap();
    assert_eq!(report.outcome, Outcome::Different);
}

#[test]
fn native_module_validation_compares_each_atom_against_its_member() {
    // `g` compares against its same-named member; the `main` atom
    // compares against `__mumei_user_main` — the emitter's rename.
    // `main` is the wrapper's call target, not a user member, so it is
    // shape-screened, never compared.
    let report = tv::validate_native_module(
        NATIVE_CALLING,
        &native_module_ir("%d = add i64 %x, 1 ret i64 %d"),
        Options::default(),
        RefinedAdmission::Reject,
    )
    .unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    assert_eq!(report.profile, tv::NATIVE_MODULE_PROFILE);
    assert_eq!(
        report
            .functions
            .iter()
            .map(|function| (function.function_name.as_str(), function.outcome))
            .collect::<Vec<_>>(),
        vec![
            ("g", Outcome::EquivalentWithinProfile),
            ("main", Outcome::EquivalentWithinProfile)
        ]
    );

    // A sabotaged helper body fails only its own entry: `main` still
    // applies the same uninterpreted `g` on both sides.
    let report = tv::validate_native_module(
        NATIVE_CALLING,
        &native_module_ir("%d = add i64 %x, 2 ret i64 %d"),
        Options::default(),
        RefinedAdmission::Reject,
    )
    .unwrap();
    assert_eq!(report.outcome, Outcome::Different);
    assert_eq!(report.functions[0].outcome, Outcome::Different);
    assert_eq!(
        report.functions[1].outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn native_module_report_names_the_declared_source_profile() {
    // A `type` declaration — even one no signature uses — makes the
    // module the refined source profile, matching the name the record
    // pins; a signature that resolves through the alias likewise names
    // it, and a plain module keeps the wrapping name.
    let const_ir = |user_main: &str| {
        pin(&format!(
            "define i64 @__mumei_user_main() {{ {user_main} }}\n\
             define i32 @main(i32 %argc, ptr %argv) {{\n\
             %result = call i64 @__mumei_user_main()\n\
             %exit_code = trunc i64 %result to i32\n\
             ret i32 %exit_code\n}}"
        ))
    };
    let declared_unused = "type Port = i64 where v >= 1 && v <= 65535;\n\
         atom main() -> i64 requires: true; ensures: result == 7; body: { 7 };";
    let report = tv::validate_native_module(
        declared_unused,
        &const_ir("ret i64 7"),
        Options::default(),
        RefinedAdmission::V1,
    )
    .unwrap();
    assert_eq!(report.source_profile, PURE_I64_REFINED_PROFILE);

    let used = "type Port = i64 where v >= 1 && v <= 65535;\n\
         atom id(p: Port) -> i64 requires: true; ensures: result == p; body: { p };\n\
         atom main() -> i64 requires: true; ensures: result == 80; body: { id(80) };";
    let used_ir = pin(
        "define i64 @id(i64 %p) { ret i64 %p }\n\
         define i64 @__mumei_user_main() { %a = call i64 @id(i64 80) ret i64 %a }\n\
         define i32 @main(i32 %argc, ptr %argv) { %result = call i64 @__mumei_user_main() %exit_code = trunc i64 %result to i32 ret i32 %exit_code }",
    );
    let report =
        tv::validate_native_module(used, &used_ir, Options::default(), RefinedAdmission::V1)
            .unwrap();
    assert_eq!(report.source_profile, PURE_I64_REFINED_PROFILE);

    let plain = tv::validate_native_module(
        NATIVE_CALLING,
        &native_module_ir("ret i64 %x"),
        Options::default(),
        RefinedAdmission::V1,
    )
    .unwrap();
    assert_eq!(plain.source_profile, PURE_I64_WRAPPING_PROFILE);
    // The strict admission refuses the declared module outright — a v1
    // record could never describe it.
    assert!(tv::validate_native_module(
        declared_unused,
        &const_ir("ret i64 7"),
        Options::default(),
        RefinedAdmission::Reject,
    )
    .is_err());
}

#[test]
fn native_module_validation_requires_main_and_its_wrapper() {
    // No `main` atom: the emitted binary has nothing to wrap.
    let helpers = "atom g(x: i64) -> i64 requires: true; ensures: true; body: { x };";
    assert!(tv::validate_native_module(
        helpers,
        &native_module_ir("ret i64 %x"),
        Options::default(),
        RefinedAdmission::Reject,
    )
    .is_err());
    // A `main` atom with inputs cannot back the zero-argument wrapper call.
    let parametrized = "atom main(x: i64) -> i64 requires: true; ensures: true; body: { x };";
    assert!(tv::validate_native_module(
        parametrized,
        &native_module_ir("ret i64 %x"),
        Options::default(),
        RefinedAdmission::Reject,
    )
    .is_err());
    // `__mumei_user_main` is the emitter-reserved rename target.
    let reserved = "atom __mumei_user_main() -> i64 requires: true; ensures: true; body: { 7 }; atom main() -> i64 requires: true; ensures: true; body: { 7 };";
    assert!(tv::validate_native_module(
        reserved,
        &native_module_ir("ret i64 %x"),
        Options::default(),
        RefinedAdmission::Reject,
    )
    .is_err());
    // The C `main` wrapper must be present with its fixed shape.
    let no_wrapper = pin(
        "define i64 @g(i64 %x) { ret i64 %x }\n\
         define i64 @__mumei_user_main() { %a = call i64 @g(i64 40) %r = add i64 %a, 2 ret i64 %r }",
    );
    assert!(tv::validate_native_module(
        NATIVE_CALLING,
        &no_wrapper,
        Options::default(),
        RefinedAdmission::Reject
    )
    .is_err());
    // A wrapper that calls a different target is rejected.
    let wrong_wrapper = pin(
        "define i64 @g(i64 %x) { ret i64 %x }\n\
         define i64 @__mumei_user_main() { %a = call i64 @g(i64 40) %r = add i64 %a, 2 ret i64 %r }\n\
         define i32 @main(i32 %argc, ptr %argv) { %result = call i64 @g(i64 0) %exit_code = trunc i64 %result to i32 ret i32 %exit_code }",
    );
    assert!(tv::validate_native_module(
        NATIVE_CALLING,
        &wrong_wrapper,
        Options::default(),
        RefinedAdmission::Reject
    )
    .is_err());
    // A missing helper member cannot even form the call-site comparison.
    let missing = pin(
        "define i64 @__mumei_user_main() { %a = call i64 @g(i64 40) %r = add i64 %a, 2 ret i64 %r }\n\
         define i32 @main(i32 %argc, ptr %argv) { %result = call i64 @__mumei_user_main() %exit_code = trunc i64 %result to i32 ret i32 %exit_code }",
    );
    assert!(tv::validate_native_module(
        NATIVE_CALLING,
        &missing,
        Options::default(),
        RefinedAdmission::Reject
    )
    .is_err());
    // Members keep external linkage.
    let internal =
        native_module_ir("ret i64 %x").replace("define i64 @g(", "define internal i64 @g(");
    assert!(tv::validate_native_module(
        NATIVE_CALLING,
        &internal,
        Options::default(),
        RefinedAdmission::Reject
    )
    .is_err());
}

#[test]
fn native_module_validation_maps_user_main_calls_back_to_main() {
    // The binary emitter rewrites a call to the `main` atom as
    // `call @__mumei_user_main`; the call-site comparison must meet it
    // under the source name — otherwise every correct caller of `main`
    // reports Different (distinct uninterpreted functions).
    let callers_main = "atom f() -> i64 requires: true; ensures: true; body: { main() + 1 }; atom main() -> i64 requires: true; ensures: true; body: { 7 };";
    let ir = pin(
        "define i64 @f() { %a = call i64 @__mumei_user_main() %r = add i64 %a, 1 ret i64 %r }\n\
         define i64 @__mumei_user_main() { ret i64 7 }\n\
         define i32 @main(i32 %argc, ptr %argv) { %result = call i64 @__mumei_user_main() %exit_code = trunc i64 %result to i32 ret i32 %exit_code }",
    );
    let report = tv::validate_native_module(
        callers_main,
        &ir,
        Options::default(),
        RefinedAdmission::Reject,
    )
    .unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    // The same rename binds the single-function route: a recursive
    // `main()` meets `call @__mumei_user_main`.
    let recursive = native_source("main() + 1");
    let ir = pin(
        "define i64 @__mumei_user_main() { %a = call i64 @__mumei_user_main() %r = add i64 %a, 1 ret i64 %r }\n\
         define i32 @main(i32 %argc, ptr %argv) { %result = call i64 @__mumei_user_main() %exit_code = trunc i64 %result to i32 ret i32 %exit_code }",
    );
    assert_eq!(
        tv::validate_native_main(&recursive, &ir, Options::default())
            .unwrap()
            .outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn native_module_validation_checks_emitted_snapshot_per_atom() {
    // End to end: the real binary emitter's multi-atom snapshot — a
    // helper plus `main` renamed to `__mumei_user_main` behind the C
    // wrapper — validates per atom.
    let items = parse_module_checked(NATIVE_CALLING).unwrap();
    let hirs: Vec<_> = items
        .iter()
        .map(|item| {
            let Item::Atom(atom) = item else {
                panic!("atom")
            };
            lower_atom_to_hir(atom)
        })
        .collect();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("native.ll");
    mumei_emit_llvm::binary::compile_atoms_to_binary_ll(&hirs, &ModuleEnv::default(), &[], &path)
        .unwrap();
    let ir = pin(&std::fs::read_to_string(&path).unwrap());
    let report = tv::validate_native_module(
        NATIVE_CALLING,
        &ir,
        Options::default(),
        RefinedAdmission::Reject,
    )
    .unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    assert_eq!(report.functions.len(), 2);
    assert!(report
        .functions
        .iter()
        .all(|function| function.outcome == Outcome::EquivalentWithinProfile));
}

#[test]
fn loop_scoped_bindings_do_not_leak_into_later_loop_phis() {
    // `let t` exists only inside the first loop's body. When it leaked
    // into `variables`, the second loop emitted
    // `phi i64 [ %lam_call, %loop.after ]` — an incoming value defined
    // in a block that does not dominate the predecessor — and LLVM
    // `verify()` rejected the merged module (breaking `run`/`build`).
    let src = "atom main() requires: true; ensures: result == 0; body: {
        let a = [1, 4, 6];
        let g0 = |v| { a[0] = v; 0 };
        let i1 = 0;
        while i1 < 2 invariant: i1 >= 0 && i1 <= 2 decreases: 2 - i1 {
            let t = call(g0, 5);
            let inner = |x| { t + x };
            let u = call(inner, 0);
            i1 = i1 + 1
        };
        let g1 = |v| { a[2] = v; 0 };
        let i2 = 0;
        while i2 < 2 invariant: i2 >= 0 && i2 <= 2 decreases: 2 - i2 {
            let t = call(g1, 8);
            i2 = i2 + 1
        };
        0
    };";
    let items = parse_module_checked(src).unwrap();
    let hirs: Vec<_> = items
        .iter()
        .map(|item| {
            let Item::Atom(atom) = item else {
                panic!("atom")
            };
            lower_atom_to_hir(atom)
        })
        .collect();
    let root = tempfile::tempdir().unwrap();
    let ll_path = root.path().join("merged.ll");
    let obj_path = root.path().join("merged.o");
    mumei_emit_llvm::binary::compile_atoms_to_binary_ll(
        &hirs,
        &ModuleEnv::default(),
        &[],
        &ll_path,
    )
    .unwrap();
    let ir = std::fs::read_to_string(&ll_path).unwrap();
    // Body-only `t`, `inner` and `u` must not produce loop phis at all
    // (the `inner` lambda marker leaves var_types too, so a later
    // lambda cannot demand its captured `t` value).
    assert!(!ir.contains("%phi_t"));
    assert!(!ir.contains("phi_inner"));
    assert!(!ir.contains("phi_u"));
    // The recorded emit runs `verify()` on the bound module — the check
    // that originally rejected this program.
    mumei_emit_llvm::codegen::compile_llvm_ir_to_object_recorded(&ll_path, &obj_path).unwrap();
}

#[test]
fn bounded_while_emitted_ir_is_equivalent_within_bound() {
    // The emitted loop's back-edge is walked up to LOOP_BOUND header
    // visits; the source is unrolled to the same bound, so inputs whose
    // loop exits within the bound compare equal.
    let src = source(
        "x: i64",
        "let n = x; while n > 0 invariant: n >= 0 decreases: n { n = n - 1 }; n",
    );
    let report = validate(&src, &emitted(&src));
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    assert_eq!(report.profile, tv::BOUNDED_LOOPS_PROFILE);
    assert_eq!(
        report.input_domain,
        "all_signed_i64_inputs_not_restricted_by_requires_and_loops_exiting_within_bound_4"
    );
    // The bound also lands as a structured field so consumers read the
    // coverage limit without parsing the domain label.
    assert_eq!(
        report.unroll_bound,
        Some(mumei_core::semantic_contract::LOOP_BOUND)
    );
}

#[test]
fn bounded_while_accumulator_emitted_ir_is_equivalent() {
    // Loop-carried writes other than the guard variable merge through
    // the same per-name `ite` as branch reassignment.
    let src = source(
        "x: i64",
        "let n = x; let s = 0; while n > 0 invariant: n >= 0 decreases: n { s = s + n; n = n - 1 }; s",
    );
    let report = validate(&src, &emitted(&src));
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    assert_eq!(report.profile, tv::BOUNDED_LOOPS_PROFILE);
}

#[test]
fn bounded_while_divergent_body_is_refuted_on_covered_inputs() {
    // `n - 2` diverges from `n - 1` for inputs whose loop still exits
    // within the bound (x = 1 leaves n = -1 instead of 0).
    let src = source(
        "x: i64",
        "let n = x; while n > 0 invariant: n >= 0 decreases: n { n = n - 1 }; n",
    );
    let other = source(
        "x: i64",
        "let n = x; while n > 0 invariant: n >= 0 decreases: n + 1 { n = n - 2 }; n",
    );
    let report = validate(&src, &emitted(&other));
    assert_eq!(report.outcome, Outcome::Different);
}

#[test]
fn while_exceeding_the_bound_stays_equivalent_on_covered_inputs() {
    // `n < 10` only exits within 4 guard evaluations for x >= 7; those
    // covered inputs still compare equivalent — deeper executions are
    // outside the declared domain, not a divergence.
    let src = source(
        "x: i64",
        "let n = x; while n < 10 invariant: n <= 10 decreases: 10 - n { n = n + 1 }; n",
    );
    let report = validate(&src, &emitted(&src));
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    assert_eq!(report.profile, tv::BOUNDED_LOOPS_PROFILE);
}

#[test]
fn while_never_exiting_within_bound_is_out_of_profile() {
    // A guard that can never fail leaves no covered input: the declared
    // domain is empty, so validation errors instead of reporting a
    // vacuous equivalence or a divergence.
    let src = source(
        "x: i64",
        "let n = x; while true invariant: true { n = n + 1 }; n",
    );
    assert!(tv::validate_pure_i64(&src, &function("ret i64 0"), Options::default()).is_err());
}

#[test]
fn cyclic_ir_still_rejected_for_loop_free_source() {
    // The bounded-cycle gate is opt-in per atom: a source without `while`
    // keeps the acyclicity requirement and a back-edge stays unsupported.
    let ir = pin("define i64 @f(i64 %x) {
        entry:
          br label %loop
        loop:
          %c = icmp sgt i64 %x, 0
          br i1 %c, label %loop, label %done
        done:
          ret i64 %x
        }");
    assert!(tv::validate_pure_i64(&source("x: i64", "x"), &ir, Options::default()).is_err());
}

#[test]
fn ir_still_looping_on_covered_input_is_inconclusive() {
    // Source exits within the bound for x > 0; this IR counts up, so
    // its path is dropped past the bound. Bound truncation cannot tell
    // divergence apart from a loop that would return the same value a
    // few iterations later — neither `Equivalent` nor `Different`.
    let src = source(
        "x: i64",
        "let n = x; while n > 0 invariant: n >= 0 decreases: n { n = n - 1 }; n",
    );
    let ir = pin("define i64 @f(i64 %x) {
        entry:
          br label %loop.header
        loop.header:
          %n = phi i64 [ %x, %entry ], [ %n2, %loop.body ]
          %c = icmp sgt i64 %n, 0
          br i1 %c, label %loop.body, label %loop.after
        loop.body:
          %n2 = add i64 %n, 1
          br label %loop.header
        loop.after:
          ret i64 %n
        }");
    let report = validate(&src, &ir);
    assert_eq!(report.outcome, Outcome::Inconclusive);
}

#[test]
fn sibling_phi_reads_the_previous_iterations_value() {
    // `%m` reads `%n` on the back-edge: per phi semantics it sees the
    // previous iteration's `%n`, so for x = 2 the loop returns 1 — a
    // sequential read of the freshly bound phi would return 0 and
    // spuriously match the source's `n`.
    let src = source(
        "x: i64",
        "let n = x; while n > 0 invariant: n >= 0 decreases: n { n = n - 1 }; n",
    );
    let ir = pin("define i64 @f(i64 %x) {
        entry:
          br label %loop.header
        loop.header:
          %n = phi i64 [ %x, %entry ], [ %n2, %loop.body ]
          %m = phi i64 [ %x, %entry ], [ %n, %loop.body ]
          %c = icmp sgt i64 %n, 0
          br i1 %c, label %loop.body, label %loop.after
        loop.body:
          %n2 = sub i64 %n, 1
          br label %loop.header
        loop.after:
          ret i64 %m
        }");
    let report = validate(&src, &ir);
    assert_eq!(report.outcome, Outcome::Different);
}

#[test]
fn defined_cancellation_named_function_stays_a_call_site() {
    // A function DEFINING `__mumei_task_group_should_cancel_current` is
    // an ordinary callee, not the bound-to-0 intrinsic: returning 1
    // exits the loop early, which the source never does.
    let src = source(
        "x: i64",
        "let n = x; while n > 0 invariant: n >= 0 decreases: n { n = n - 1 }; n",
    );
    let ir = pin("define i64 @__mumei_task_group_should_cancel_current() {
        entry:
          ret i64 1
        }
        define i64 @f(i64 %x) {
        entry:
          br label %loop.header
        loop.header:
          %n = phi i64 [ %x, %entry ], [ %n2, %loop.body ]
          %poll = call i64 @__mumei_task_group_should_cancel_current()
          %cancelled = icmp ne i64 %poll, 0
          br i1 %cancelled, label %loop.after, label %loop.cond
        loop.cond:
          %c = icmp sgt i64 %n, 0
          br i1 %c, label %loop.body, label %loop.after
        loop.body:
          %n2 = sub i64 %n, 1
          br label %loop.header
        loop.after:
          ret i64 %n
        }");
    let report = validate(&src, &ir);
    assert_eq!(report.outcome, Outcome::Different);
}

#[test]
fn unreached_while_owes_no_exit() {
    // The loop is inside the `x > 0` branch: its bound obligation is
    // `reached => exits within bound`, so inputs x <= 0 that skip the
    // loop stay covered and compare equivalent.
    let src = source(
        "x: i64",
        "let n = x; if x > 0 { while true invariant: true { n = n + 1 }; n } else { n }; n",
    );
    let report = validate(&src, &emitted(&src));
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    assert_eq!(report.profile, tv::BOUNDED_LOOPS_PROFILE);
}

const COMPOSITE_SOURCE: &str = "struct S { on: bool, x: i64 }\n\
    atom pick(s: S) -> i64 requires: true; ensures: result == 0 || result == s.x; body: { if s.on { s.x } else { 0 } };\n\
    atom fwd(t: S, k: i64) -> i64 requires: true; ensures: true; body: { pick(t) + k };\n\
    atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };";

/// The emitter's own native snapshot of `COMPOSITE_SOURCE`: each struct
/// parameter is one `{ i64, i64 }` formal read by `extractvalue` and
/// passed through to `pick` unchanged.
const COMPOSITE_IR: &str = r#"source_filename = "mumei_merged"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128"
target triple = "x86_64-pc-linux-gnu"
define i64 @pick({ i64, i64 } %0) {
entry:
  %s.on = extractvalue { i64, i64 } %0, 0
  %if_cond = icmp ne i64 %s.on, 0
  br i1 %if_cond, label %then, label %else
then:
  %s.x = extractvalue { i64, i64 } %0, 1
  br label %merge
else:
  br label %merge
merge:
  %if_result = phi i64 [ %s.x, %then ], [ 0, %else ]
  ret i64 %if_result
}
define i64 @fwd({ i64, i64 } %0, i64 %1) {
entry:
  %call_pick = call i64 @pick({ i64, i64 } %0)
  %add_tmp = add i64 %call_pick, %1
  ret i64 %add_tmp
}
define i64 @__mumei_user_main() {
entry:
  ret i64 0
}
define i32 @main(i32 %0, ptr %1) {
entry:
  %result = call i64 @__mumei_user_main()
  %exit_code = trunc i64 %result to i32
  ret i32 %exit_code
}
"#;

fn composite(source: &str, ir: &str) -> Result<tv::ModuleValidationReport, tv::ValidationError> {
    tv::validate_native_module(
        source,
        ir,
        Options::default(),
        mumei_core::semantic_contract::RefinedAdmission::Composite,
    )
}

#[test]
fn composite_struct_formals_pair_with_flattened_field_slots() {
    let report = composite(COMPOSITE_SOURCE, COMPOSITE_IR).unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    assert_eq!(
        report.source_profile,
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE
    );
    // Field order is positional: reading slot 1 where the source reads
    // `s.on` (slot 0) is a different member, not a renamed one.
    let swapped = COMPOSITE_IR
        .replace(
            "%s.on = extractvalue { i64, i64 } %0, 0",
            "%s.on = extractvalue { i64, i64 } %0, 1",
        )
        .replace(
            "%s.x = extractvalue { i64, i64 } %0, 1",
            "%s.x = extractvalue { i64, i64 } %0, 0",
        );
    assert_eq!(
        composite(COMPOSITE_SOURCE, &swapped).unwrap().outcome,
        Outcome::Different
    );
    let one_swapped = COMPOSITE_IR.replace(
        "%s.x = extractvalue { i64, i64 } %0, 1",
        "%s.x = extractvalue { i64, i64 } %0, 0",
    );
    assert_eq!(
        composite(COMPOSITE_SOURCE, &one_swapped).unwrap().outcome,
        Outcome::Different
    );
    // The pass-through call result is compared as an application on the
    // flattened field arguments: reusing it in place of `k` differs.
    let wrong_call = COMPOSITE_IR.replace(
        "%call_pick = call i64 @pick({ i64, i64 } %0)\n  %add_tmp = add i64 %call_pick, %1",
        "%call_pick = call i64 @pick({ i64, i64 } %0)\n  %add_tmp = add i64 %call_pick, %call_pick",
    );
    assert_eq!(
        composite(COMPOSITE_SOURCE, &wrong_call).unwrap().outcome,
        Outcome::Different
    );
}

#[test]
fn composite_aggregates_outside_struct_formal_reads_and_pass_through_fail_closed() {
    let pick_body = "  %s.on = extractvalue { i64, i64 } %0, 0\n  %if_cond = icmp ne i64 %s.on, 0\n  br i1 %if_cond, label %then, label %else";
    for (from, to) in [
        // an aggregate built in the body, not a formal
        (
            pick_body,
            "  %b = insertvalue { i64, i64 } %0, i64 1, 0\n  %s.on = extractvalue { i64, i64 } %b, 0\n  %if_cond = icmp ne i64 %s.on, 0\n  br i1 %if_cond, label %then, label %else",
        ),
        // a constant aggregate
        (
            "%s.on = extractvalue { i64, i64 } %0, 0",
            "%s.on = extractvalue { i64, i64 } { i64 1, i64 0 }, 0",
        ),
        // an aggregate merged by select
        (
            pick_body,
            "  %m = select i1 true, { i64, i64 } %0, { i64, i64 } %0\n  %s.on = extractvalue { i64, i64 } %m, 0\n  %if_cond = icmp ne i64 %s.on, 0\n  br i1 %if_cond, label %then, label %else",
        ),
        // nested, mixed-width and packed formals
        ("define i64 @pick({ i64, i64 } %0)", "define i64 @pick({ { i64 }, i64 } %0)"),
        ("define i64 @pick({ i64, i64 } %0)", "define i64 @pick({ i32, i64 } %0)"),
        ("define i64 @pick({ i64, i64 } %0)", "define i64 @pick(<{ i64, i64 }> %0)"),
        // a formal whose arity differs from the struct's field count
        ("define i64 @pick({ i64, i64 } %0)", "define i64 @pick({ i64, i64, i64 } %0)"),
        // a struct parameter lowered as two scalar formals instead
        ("define i64 @pick({ i64, i64 } %0)", "define i64 @pick(i64 %0, i64 %9)"),
    ] {
        let ir = COMPOSITE_IR.replace(from, to);
        assert_ne!(ir, COMPOSITE_IR, "probe did not apply: {to}");
        assert!(
            !matches!(
                composite(COMPOSITE_SOURCE, &ir).map(|report| report.outcome),
                Ok(Outcome::EquivalentWithinProfile)
            ),
            "aggregate probe accepted: {to}"
        );
    }
    // A scalar source result never pairs with an aggregate return.
    let returning = COMPOSITE_IR
        .replace(
            "define i64 @fwd({ i64, i64 } %0, i64 %1) {",
            "define { i64, i64 } @fwd({ i64, i64 } %0, i64 %1) {",
        )
        .replace(
            "  %add_tmp = add i64 %call_pick, %1\n  ret i64 %add_tmp",
            "  %add_tmp = add i64 %call_pick, %1\n  ret { i64, i64 } %0",
        );
    assert_ne!(returning, COMPOSITE_IR);
    assert!(composite(COMPOSITE_SOURCE, &returning).is_err());
    // A scalar source parameter never pairs with an aggregate formal.
    let scalar = COMPOSITE_SOURCE
        .replace("atom fwd(t: S, k: i64)", "atom fwd(t: i64, k: i64)")
        .replace("pick(t) + k", "t + k");
    assert!(composite(&scalar, COMPOSITE_IR).is_err());
}

const STRUCT_RESULT_SOURCE: &str = "struct P { x: i64, y: i64 }\n\
    atom make(a: i64) -> P requires: true; ensures: result.x == a; body: { P { x: a, y: a + 1 } };\n\
    atom usep(a: i64) -> i64 requires: true; ensures: true; body: { make(a).y };\n\
    atom initlet(a: i64) -> i64 requires: true; ensures: result == a; body: { let q = P { x: 5, y: a }; q.y };\n\
    atom fwd(a: i64) -> P requires: true; ensures: result.x == a; body: { make(a) };\n\
    atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };";

/// The emitter's own native snapshot of `STRUCT_RESULT_SOURCE`: a struct
/// result is an `insertvalue` chain returned as `{ i64, i64 }` (folded
/// into a constant base where a field is a literal), and a call's struct
/// result is read by `extractvalue` or returned unchanged.
const STRUCT_RESULT_IR: &str = r#"source_filename = "mumei_merged"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128"
target triple = "x86_64-pc-linux-gnu"
define { i64, i64 } @make(i64 %0) {
entry:
  %add_tmp = add i64 %0, 1
  %struct_x = insertvalue { i64, i64 } undef, i64 %0, 0
  %struct_y = insertvalue { i64, i64 } %struct_x, i64 %add_tmp, 1
  ret { i64, i64 } %struct_y
}
define i64 @usep(i64 %0) {
entry:
  %call_make = call { i64, i64 } @make(i64 %0)
  %nested.y = extractvalue { i64, i64 } %call_make, 1
  ret i64 %nested.y
}
define i64 @initlet(i64 %0) {
entry:
  %struct_y = insertvalue { i64, i64 } { i64 5, i64 undef }, i64 %0, 1
  %q.y = extractvalue { i64, i64 } %struct_y, 1
  ret i64 %q.y
}
define { i64, i64 } @fwd(i64 %0) {
entry:
  %call_make = call { i64, i64 } @make(i64 %0)
  ret { i64, i64 } %call_make
}
define i64 @__mumei_user_main() {
entry:
  ret i64 0
}
define i32 @main(i32 %0, ptr %1) {
entry:
  %result = call i64 @__mumei_user_main()
  %exit_code = trunc i64 %result to i32
  ret i32 %exit_code
}
"#;

fn struct_results(
    source: &str,
    ir: &str,
) -> Result<tv::ModuleValidationReport, tv::ValidationError> {
    tv::validate_native_module(
        source,
        ir,
        Options::default(),
        mumei_core::semantic_contract::RefinedAdmission::CompositeV3,
    )
}

#[test]
fn struct_results_pair_returned_aggregates_with_field_slots() {
    let report = struct_results(STRUCT_RESULT_SOURCE, STRUCT_RESULT_IR).unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    assert_eq!(
        report.source_profile,
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V3
    );
    for (from, to) in [
        // returned fields written to each other's slots
        (
            "%struct_x = insertvalue { i64, i64 } undef, i64 %0, 0\n  %struct_y = insertvalue { i64, i64 } %struct_x, i64 %add_tmp, 1",
            "%struct_x = insertvalue { i64, i64 } undef, i64 %0, 1\n  %struct_y = insertvalue { i64, i64 } %struct_x, i64 %add_tmp, 0",
        ),
        // a call result's slot read at the wrong index
        (
            "%nested.y = extractvalue { i64, i64 } %call_make, 1",
            "%nested.y = extractvalue { i64, i64 } %call_make, 0",
        ),
        // the folded literal's constant slot read instead of the written one
        (
            "%q.y = extractvalue { i64, i64 } %struct_y, 1",
            "%q.y = extractvalue { i64, i64 } %struct_y, 0",
        ),
        // a forwarded call result replaced by a fresh literal
        (
            "  %call_make = call { i64, i64 } @make(i64 %0)\n  ret { i64, i64 } %call_make",
            "  %call_make = call { i64, i64 } @make(i64 %0)\n  ret { i64, i64 } { i64 0, i64 0 }",
        ),
    ] {
        let ir = STRUCT_RESULT_IR.replace(from, to);
        assert_ne!(ir, STRUCT_RESULT_IR, "probe did not apply: {to}");
        assert_eq!(
            struct_results(STRUCT_RESULT_SOURCE, &ir).unwrap().outcome,
            Outcome::Different,
            "probe not different: {to}"
        );
    }
    for (from, to) in [
        // a returned slot no `insertvalue` wrote is never guessed
        (
            "  %struct_y = insertvalue { i64, i64 } %struct_x, i64 %add_tmp, 1\n  ret { i64, i64 } %struct_y",
            "  %struct_y = insertvalue { i64, i64 } %struct_x, i64 %add_tmp, 1\n  ret { i64, i64 } %struct_x",
        ),
        // reading the undefined slot of a folded literal
        (
            "%struct_y = insertvalue { i64, i64 } { i64 5, i64 undef }, i64 %0, 1\n  %q.y = extractvalue { i64, i64 } %struct_y, 1",
            "%struct_y = insertvalue { i64, i64 } { i64 5, i64 undef }, i64 %0, 0\n  %q.y = extractvalue { i64, i64 } %struct_y, 1",
        ),
        // aggregates merged by select stay outside
        (
            "  %call_make = call { i64, i64 } @make(i64 %0)\n  ret { i64, i64 } %call_make",
            "  %call_make = call { i64, i64 } @make(i64 %0)\n  %m = select i1 true, { i64, i64 } %call_make, { i64, i64 } %call_make\n  ret { i64, i64 } %m",
        ),
        // a nested or wider returned aggregate
        (
            "define { i64, i64 } @make(i64 %0) {",
            "define { i64, i64, i64 } @make(i64 %0) {",
        ),
        // a struct result returned as a scalar
        (
            "  %call_make = call { i64, i64 } @make(i64 %0)\n  ret { i64, i64 } %call_make",
            "  %call_make = call { i64, i64 } @make(i64 %0)\n  %f = extractvalue { i64, i64 } %call_make, 0\n  ret i64 %f",
        ),
    ] {
        let ir = STRUCT_RESULT_IR.replace(from, to);
        assert_ne!(ir, STRUCT_RESULT_IR, "probe did not apply: {to}");
        assert!(
            !matches!(
                struct_results(STRUCT_RESULT_SOURCE, &ir).map(|report| report.outcome),
                Ok(Outcome::EquivalentWithinProfile)
            ),
            "struct result probe accepted: {to}"
        );
    }
    // Below composite v3 the struct-valued source itself is refused.
    assert!(composite(STRUCT_RESULT_SOURCE, STRUCT_RESULT_IR).is_err());
}

const BOOL_ENUM_TRANSFER_SOURCE: &str = "enum Tag { Some(b: bool), None } atom fwd(t: Tag) -> Tag requires: true; ensures: true; body: { t }; atom main() -> i64 requires: true; ensures: true; body: { 0 };";

fn emitted_module(items: &[Item]) -> String {
    let mut env = ModuleEnv::default();
    let mut lowered = Vec::new();
    for item in items {
        match item {
            Item::TypeDef(typedef) => env.register_type(typedef),
            Item::StructDef(struct_def) => env.register_struct(struct_def),
            Item::EnumDef(enum_def) => env.register_enum(enum_def),
            Item::Atom(atom) => {
                env.register_atom(atom);
                lowered.push(mumei_core::hir::lower_atom_to_hir_with_env(
                    atom,
                    Some(&env),
                ));
            }
            _ => {}
        }
    }
    let context = mumei_emit_llvm::LlvmContext::create();
    let module = context.create_module("enum_transfer");
    mumei_emit_llvm::codegen::compile_atoms_into_module(&context, &module, &lowered, &env, &[])
        .unwrap();
    // `compile_atoms_into_module` keeps the user `main` name; the
    // bundle emitter renames it to `__mumei_user_main` and adds the
    // i32 wrapper — do the same here.
    let ir = module
        .print_to_string()
        .to_string()
        .replace("@main(", "@__mumei_user_main(");
    let ir = format!(
        "{ir}\ndefine i32 @main(i32 %0, ptr %1) {{
entry:
  %result = call i64 @__mumei_user_main()
  %exit_code = trunc i64 %result to i32
  ret i32 %exit_code
}}\n"
    );
    pin(&ir)
}

#[test]
fn composite_v6_bool_padding_slots_compare_against_normalized_source_values() {
    // Under `None` the source reads the padding slot as `v != 0`; the
    // emitter passes the raw i64 through, so the domain must keep it
    // ≤1 for a pass-through to compare equal.
    let items = parse_module_checked(BOOL_ENUM_TRANSFER_SOURCE).unwrap();
    let ir = emitted_module(&items);
    let report = tv::validate_native_module(
        BOOL_ENUM_TRANSFER_SOURCE,
        &ir,
        Options::default(),
        RefinedAdmission::CompositeV6,
    )
    .unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
}

const BRANCH_MERGED_COMPOSITE_SOURCE: &str = "struct P { x: i64, y: i64 }\nenum Shape { Dot, Edge(w: i64) }\natom pick(p: P, c: i64) -> P requires: c >= 0 && p.x >= 0; ensures: result.x >= 0; body: { if c == 0 { p } else { P { x: c, y: c } } };\natom pick_enum(s: Shape, c: i64) -> Shape requires: c >= 0; ensures: true; body: { if c == 0 { s } else { Shape::Edge(c) } };\natom main() -> i64 requires: true; ensures: true; body: { let q = pick(P { x: 1, y: 2 }, 3); q.x };";

#[test]
fn composite_v7_branch_merged_aggregates_validate_against_phi_ir() {
    // composite v7 lowers a branch-merged struct/enum to a `{i64, ...}`
    // phi: the validator must follow the merge per slot, so the real
    // emitter output compares EquivalentWithinProfile.
    let items = parse_module_checked(BRANCH_MERGED_COMPOSITE_SOURCE).unwrap();
    let ir = emitted_module(&items);
    assert!(
        ir.contains("phi { i64, i64 }"),
        "emitter did not phi a composite:\n{ir}"
    );
    let report = tv::validate_native_module(
        BRANCH_MERGED_COMPOSITE_SOURCE,
        &ir,
        Options::default(),
        RefinedAdmission::CompositeV7,
    )
    .unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    // Reading the wrong slot of the merged aggregate refutes the
    // comparison: `main` must take `q.x`, not `q.y`.
    let swapped = ir.replace(
        "%q.x = extractvalue { i64, i64 } %call_pick, 0",
        "%q.x = extractvalue { i64, i64 } %call_pick, 1",
    );
    assert_ne!(swapped, ir, "probe did not apply");
    assert_eq!(
        tv::validate_native_module(
            BRANCH_MERGED_COMPOSITE_SOURCE,
            &swapped,
            Options::default(),
            RefinedAdmission::CompositeV7,
        )
        .unwrap()
        .outcome,
        Outcome::Different
    );
}

const SHORT_VARIANT_SOURCE: &str = "enum Shape { Dot, Edge(w: i64) }\natom mk() -> Shape requires: true; ensures: true; body: { Shape::Dot };\natom main() -> i64 requires: true; ensures: true; body: { 0 };";

#[test]
fn composite_v6_short_variant_zero_fills_unwritten_payload_slots() {
    // `Dot` carries no payload, so slot 1 of the `{i64, i64}` union is
    // left unwritten by the variant init. The source model pads it with
    // zero; an `undef` starting value would fail the slot comparison.
    let items = parse_module_checked(SHORT_VARIANT_SOURCE).unwrap();
    let ir = emitted_module(&items);
    let report = tv::validate_native_module(
        SHORT_VARIANT_SOURCE,
        &ir,
        Options::default(),
        RefinedAdmission::CompositeV6,
    )
    .unwrap();
    assert_eq!(
        report.outcome,
        Outcome::EquivalentWithinProfile,
        "short-variant enum padding did not recheck:\n{ir}"
    );
}

// The source keeps its own bounded scalar loop — it is what lets the
// IR side walk a cycle at all — while the composite `p`/`q` carriers
// exist only in the IR: `q` tracks the previous iteration's `p`, i.e.
// `max(0, s - 1)` where `s` counts iterations.
const LOOP_SIBLING_PHI_SOURCE: &str = "atom f(n: i64) -> i64 requires: n >= 0 && n <= 4; ensures: true; body: { let i = 0; let s = 0; while i < n invariant: i >= 0 && i <= n decreases: n - i { s = s + 1; i = i + 1 }; if s == 0 { 0 } else { s - 1 } };\natom main() -> i64 requires: true; ensures: true; body: { 0 };";

const LOOP_SIBLING_PHI_IR: &str = r#"define i64 @f(i64 %0) {
entry:
  br label %header
header:
  %p = phi { i64, i64 } [ { i64 0, i64 0 }, %entry ], [ %pn, %latch ]
  %q = phi { i64, i64 } [ { i64 0, i64 0 }, %entry ], [ %p, %latch ]
  %i = phi i64 [ 0, %entry ], [ %in, %latch ]
  %cond = icmp slt i64 %i, %0
  br i1 %cond, label %latch, label %exit
latch:
  %px = extractvalue { i64, i64 } %p, 0
  %px1 = add i64 %px, 1
  %pn = insertvalue { i64, i64 } %p, i64 %px1, 0
  %in = add i64 %i, 1
  br label %header
exit:
  %qx = extractvalue { i64, i64 } %q, 0
  ret i64 %qx
}
define i64 @__mumei_user_main() {
entry:
  ret i64 0
}
define i32 @main(i32 %0, ptr %1) {
entry:
  %result = call i64 @__mumei_user_main()
  %exit_code = trunc i64 %result to i32
  ret i32 %exit_code
}
"#;

#[test]
fn composite_v7_loop_aggregate_phi_reads_sibling_phi_before_commit() {
    // `%q`'s latch-edge incoming is the sibling header phi `%p` itself.
    // LLVM phis commit together, so `%q` must see `%p`'s value for the
    // current iteration — not the freshly resolved `%pn` slots.
    let ir = pin(LOOP_SIBLING_PHI_IR);
    let report = tv::validate_native_module(
        LOOP_SIBLING_PHI_SOURCE,
        &ir,
        Options::default(),
        RefinedAdmission::CompositeV7,
    )
    .unwrap();
    assert_eq!(
        report.outcome,
        Outcome::EquivalentWithinProfile,
        "loop sibling-phi comparison did not recheck"
    );
    // If `%q` instead read `%p`'s next-iteration value the comparison
    // must fail — that is exactly the eager-commit mistake this guards.
    let shifted = ir.replace(
        "  %q = phi { i64, i64 } [ { i64 0, i64 0 }, %entry ], [ %p, %latch ]\n",
        "  %q = phi { i64, i64 } [ { i64 0, i64 0 }, %entry ], [ %pn, %latch ]\n",
    );
    assert_ne!(shifted, ir, "probe did not apply");
    assert_eq!(
        tv::validate_native_module(
            LOOP_SIBLING_PHI_SOURCE,
            &shifted,
            Options::default(),
            RefinedAdmission::CompositeV7,
        )
        .unwrap()
        .outcome,
        Outcome::Different
    );
}

const NESTED_COMPOSITE_SOURCE: &str = "struct Inner { a: i64, b: i64 }\nstruct Outer { inner: Inner, c: i64 }\natom read_inner(o: Outer) -> i64 requires: true; ensures: true; body: { o.inner.a };\natom set_inner(o: Outer, w: Inner) -> Outer requires: true; ensures: true; body: { o.inner = w; o };\natom main() -> i64 requires: true; ensures: true; body: { let q = read_inner(Outer { inner: Inner { a: 3, b: 4 }, c: 5 }); q };";

#[test]
fn composite_v8_nested_struct_aggregates_validate_and_refute() {
    // composite v8 flattens nested struct fields into leaf slots:
    // `Outer` lowers to `{i64, i64, i64}` and `o.inner = w` emits an
    // aggregate `insertvalue` at index 0. The validator must compare
    // leaf ranges, not single slots.
    let items = parse_module_checked(NESTED_COMPOSITE_SOURCE).unwrap();
    let ir = emitted_module(&items);
    let report = tv::validate_native_module(
        NESTED_COMPOSITE_SOURCE,
        &ir,
        Options::default(),
        RefinedAdmission::CompositeV8,
    )
    .unwrap();
    assert_eq!(
        report.outcome,
        Outcome::EquivalentWithinProfile,
        "nested aggregate module must validate\n{ir}"
    );
    // Reading the wrong leaf of the nested aggregate refutes the
    // comparison: `o.inner.a` is index 0, `o.inner.b` is index 1.
    let swapped = ir.replacen(
        "extractvalue { i64, i64 } %o.inner, 0",
        "extractvalue { i64, i64 } %o.inner, 1",
        1,
    );
    assert_ne!(swapped, ir, "probe did not apply:\n{ir}");
    assert_eq!(
        tv::validate_native_module(
            NESTED_COMPOSITE_SOURCE,
            &swapped,
            Options::default(),
            RefinedAdmission::CompositeV8,
        )
        .unwrap()
        .outcome,
        Outcome::Different
    );
}

#[test]
fn v26_array_parameter_abi_is_validated_and_unsupported_ir_fails_closed() {
    let source =
        "atom read_at(a: [i64], i: i64) -> i64 requires: true; ensures: true; body: { a[i] };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };";
    let items = parse_module_checked(source).unwrap();
    let ir = emitted_module(&items);
    let validate = |llvm_ir: &str| {
        tv::validate_native_module(
            source,
            llvm_ir,
            Options::default(),
            RefinedAdmission::CompositeV26,
        )
    };
    let report = validate(&ir).unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    assert!(report
        .input_domain
        .ends_with("_with_read_only_i64_array_params_nonnegative_len"));
    assert!(!report.input_domain.contains("bool_params"));
    assert!(ir.contains("define i64 @read_at({ i64, ptr }"));
    assert!(tv::validate_native_module(
        source,
        &ir,
        Options::default(),
        RefinedAdmission::CompositeV25,
    )
    .is_err());

    let mixed_source = "atom read_at(a: [i64], i: i64) -> i64 requires: true; ensures: true; body: { a[i] };\n\
         atom select(flag: bool) -> i64 requires: true; ensures: true; body: { if flag { 1 } else { 0 } };\n\
         atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };";
    let mixed_items = parse_module_checked(mixed_source).unwrap();
    let mixed_ir = emitted_module(&mixed_items);
    let mixed_report = tv::validate_native_module(
        mixed_source,
        &mixed_ir,
        Options::default(),
        RefinedAdmission::CompositeV26,
    )
    .unwrap();
    assert!(mixed_report
        .input_domain
        .contains("bool_params_restricted_to_0_or_1"));
    assert!(mixed_report
        .input_domain
        .ends_with("_with_read_only_i64_array_params_nonnegative_len"));

    let replace_read_at = |llvm_ir: &str, replacement: &str| {
        let start = llvm_ir.find("define i64 @read_at(").unwrap();
        let end = start + llvm_ir[start..].find("\n}").unwrap() + 2;
        format!("{}{}{}", &llvm_ir[..start], replacement, &llvm_ir[end..])
    };
    let wrong_bound = ir.replacen("icmp slt i64", "icmp sle i64", 1);
    assert_ne!(
        wrong_bound, ir,
        "array bounds comparison probe did not apply"
    );
    assert_ne!(
        validate(&wrong_bound).unwrap().outcome,
        Outcome::EquivalentWithinProfile
    );

    let direct_load = replace_read_at(
        &ir,
        "define i64 @read_at({ i64, ptr } %a, i64 %i) {\n\
           %data = extractvalue { i64, ptr } %a, 1\n\
           %cell = getelementptr i64, ptr %data, i64 %i\n\
           %value = load i64, ptr %cell\n\
           ret i64 %value\n}",
    );
    assert_eq!(
        validate(&direct_load).unwrap().outcome,
        Outcome::UndefinedBehavior
    );

    let store = replace_read_at(
        &ir,
        "define i64 @read_at({ i64, ptr } %a, i64 %i) {\n\
           %data = extractvalue { i64, ptr } %a, 1\n\
           %cell = getelementptr i64, ptr %data, i64 %i\n\
           store i64 0, ptr %cell\n\
           ret i64 0\n}",
    );
    assert!(validate(&store)
        .unwrap_err()
        .to_string()
        .contains("unsupported use of array parameter memory"));

    let wrong_formal = replace_read_at(
        &ir,
        "define i64 @read_at(i64 %a, i64 %i) {\n\
           ret i64 0\n}",
    );
    assert!(validate(&wrong_formal)
        .unwrap_err()
        .to_string()
        .contains("LLVM array parameter type mismatch"));
}

const BOUNDED_ARRAY_SOURCE: &str = "atom sum() -> i64 requires: true; ensures: true; body: { let a = [1, 2, 3]; a[1] = 9; a[0] + a[1] + a[2] };\n\
     atom pick(a0: i64, i: i64) -> i64 requires: 0 <= i && i < 3; ensures: true; body: { let a = [a0, 20, 30]; a[i] = 7; a[i] };\n\
     atom main() -> i64 requires: true; ensures: true; body: { sum() + pick(1, 0) };";

#[test]
fn bounded_memory_arrays_validate_and_refute() {
    // composite v11 admits `let`-bound stack arrays: the emitter lowers
    // them to an entry-block `alloca [N x i64]` plus gep/load/store,
    // which the bounded-memory whitelist and provenance map encode.
    let items = parse_module_checked(BOUNDED_ARRAY_SOURCE).unwrap();
    let ir = emitted_module(&items);
    let report = tv::validate_native_module(
        BOUNDED_ARRAY_SOURCE,
        &ir,
        Options::default(),
        RefinedAdmission::CompositeV11,
    )
    .unwrap();
    assert_eq!(
        report.outcome,
        Outcome::EquivalentWithinProfile,
        "bounded-memory module must validate\n{ir}"
    );
    assert_eq!(report.profile, tv::BOUNDED_MEMORY_NATIVE_MODULE_PROFILE);
}

#[test]
fn bounded_memory_arrays_need_v11_and_pointer_escapes_fail_closed() {
    let items = parse_module_checked(BOUNDED_ARRAY_SOURCE).unwrap();
    let ir = emitted_module(&items);
    // Arrays are a v11 widening — under composite v10 the source itself
    // is out of profile.
    assert!(tv::validate_native_module(
        BOUNDED_ARRAY_SOURCE,
        &ir,
        Options::default(),
        RefinedAdmission::CompositeV10,
    )
    .is_err());
    // A stored value that does not match the source's cell refutes.
    let stored = ir.replacen("store i64 9", "store i64 8", 1);
    assert_ne!(stored, ir, "probe did not apply:\n{ir}");
    assert_eq!(
        tv::validate_native_module(
            BOUNDED_ARRAY_SOURCE,
            &stored,
            Options::default(),
            RefinedAdmission::CompositeV11,
        )
        .unwrap()
        .outcome,
        Outcome::Different
    );
    // A pointer operand outside the provenance-preserving slots —
    // here an address compared by `icmp` — fails the whitelist even
    // under bounded memory.
    let alloca = ir.find("= alloca [").expect("probe needs an alloca");
    let name_end = ir[..alloca]
        .rfind('%')
        .expect("probe needs the alloca name");
    let name_start = ir[..name_end].rfind([' ', '\n']).map_or(0, |i| i + 1);
    let alloca_name = &ir[name_start..alloca + 1];
    let escaped = ir.replacen(
        " ret i64",
        &format!(" %ptr_eq = icmp eq ptr {alloca_name}, {alloca_name}\n ret i64"),
        1,
    );
    assert_ne!(escaped, ir, "probe did not apply:\n{ir}");
    assert!(tv::validate_native_module(
        BOUNDED_ARRAY_SOURCE,
        &escaped,
        Options::default(),
        RefinedAdmission::CompositeV11,
    )
    .is_err());
}

const BOUNDED_ALIAS_SOURCE: &str = "atom alias() -> i64 requires: true; ensures: true; body: { let a = [1, 2]; let b = a; b[0] = 9; a[0] + a[1] };\natom main() -> i64 requires: true; ensures: true; body: { alias() };";

#[test]
fn bounded_memory_alias_write_reaches_every_alias() {
    // `let b = a` copies the array's fat pointer — the emitted code
    // shares one data pointer, so `b[0] = 9` must reach `a[0]` in the
    // source model too (the value is 9 + 2 = 11 on both sides).
    let items = parse_module_checked(BOUNDED_ALIAS_SOURCE).unwrap();
    let ir = emitted_module(&items);
    let report = tv::validate_native_module(
        BOUNDED_ALIAS_SOURCE,
        &ir,
        Options::default(),
        RefinedAdmission::CompositeV11,
    )
    .unwrap();
    assert_eq!(
        report.outcome,
        Outcome::EquivalentWithinProfile,
        "aliased array write must validate\n{ir}"
    );
}

const BOUNDED_SHADOWED_ALIAS_SOURCE: &str = "atom shadow(c: i64) -> i64 requires: true; ensures: true; body: { let a = [1, 2]; let b = a; if c > 0 { let a = [7, 8]; b[0] = 9; 0 } else { 0 }; a[0] + a[1] };\n\
     atom main() -> i64 requires: true; ensures: true; body: { shadow(1) };";

#[test]
fn bounded_memory_shadowed_name_still_shares_the_backing_store() {
    // A block `let` shadows the array name inside a branch, but `b`
    // still points at the original backing: the emitted code writes
    // through the shared data pointer, so `b[0] = 9` reaches the outer
    // `a` — the source model must merge that write too.
    let items = parse_module_checked(BOUNDED_SHADOWED_ALIAS_SOURCE).unwrap();
    let ir = emitted_module(&items);
    let report = tv::validate_native_module(
        BOUNDED_SHADOWED_ALIAS_SOURCE,
        &ir,
        Options::default(),
        RefinedAdmission::CompositeV11,
    )
    .unwrap();
    assert_eq!(
        report.outcome,
        Outcome::EquivalentWithinProfile,
        "shadowed-array alias write must validate\n{ir}"
    );
}

#[test]
fn bounded_memory_byte_indexed_gep_fails_closed() {
    // A `gep i8` indexes bytes, not cells — the source element type
    // gate must fail closed on it.
    let items = parse_module_checked(BOUNDED_ARRAY_SOURCE).unwrap();
    let ir = emitted_module(&items);
    let byte = ir.replacen("getelementptr i64, ptr", "getelementptr i8, ptr", 1);
    assert_ne!(byte, ir, "probe did not apply:\n{ir}");
    assert!(tv::validate_native_module(
        BOUNDED_ARRAY_SOURCE,
        &byte,
        Options::default(),
        RefinedAdmission::CompositeV11,
    )
    .is_err());
}

const BOUNDED_ZERO_SOURCE: &str = "atom zero() -> i64 requires: true; ensures: true; body: { let a = [0, 5]; a[0] };\natom main() -> i64 requires: true; ensures: true; body: { zero() };";

#[test]
fn bounded_memory_unwritten_cell_is_not_silently_zero() {
    // A dropped initialisation store must not pass: the unconstrained
    // backing array yields an arbitrary cell where a zero-filled base
    // would silently reproduce the source's 0.
    let items = parse_module_checked(BOUNDED_ZERO_SOURCE).unwrap();
    let ir = emitted_module(&items);
    let dropped = ir.replacen("  store i64 0, ptr %lit_slot, align 4\n", "", 1);
    assert_ne!(dropped, ir, "probe did not apply:\n{ir}");
    assert_eq!(
        tv::validate_native_module(
            BOUNDED_ZERO_SOURCE,
            &dropped,
            Options::default(),
            RefinedAdmission::CompositeV11,
        )
        .unwrap()
        .outcome,
        Outcome::Different
    );
}

#[test]
fn bounded_memory_out_of_bounds_pair_is_ub_not_equivalent() {
    // An out-of-bounds store+read pair agrees with the source on every
    // value — 1 + 9 + 3 on both sides — but the access itself is UB,
    // which the per-alloca bound obligation must flag.
    let items = parse_module_checked(BOUNDED_ARRAY_SOURCE).unwrap();
    let ir = emitted_module(&items);
    let oob = ir.replacen(
        "getelementptr i64, ptr %arrlit_data, i64 1",
        "getelementptr i64, ptr %arrlit_data, i64 5",
        2,
    );
    assert_ne!(oob, ir, "probe did not apply:\n{ir}");
    assert_eq!(
        tv::validate_native_module(
            BOUNDED_ARRAY_SOURCE,
            &oob,
            Options::default(),
            RefinedAdmission::CompositeV11,
        )
        .unwrap()
        .outcome,
        Outcome::UndefinedBehavior
    );
}

const F64_NATIVE_MODULE: &str = "enum Rate { Fixed(f64), None }\n\
     atom copy(a: f64) -> f64 requires: true; ensures: true; body: { a };\n\
     atom none_rate() -> Rate requires: true; ensures: true; body: { Rate::None };\n\
     atom main() -> i64 requires: true; ensures: result == 1; body: { 1 };";

fn f64_module_ir(members: &str) -> String {
    pin(&format!(
        "{members}\n\
         define i64 @__mumei_user_main() {{ ret i64 1 }}\n\
         define i32 @main(i32 %argc, ptr %argv) {{\n\
         %result = call i64 @__mumei_user_main()\n\
         %exit_code = trunc i64 %result to i32\n\
         ret i32 %exit_code\n}}"
    ))
}

#[test]
fn f64_leaf_signature_kinds_pin_the_abi() {
    // The good module: `copy`'s f64 leaf is a `double` formal, and the
    // empty `Rate::None` variant lowers to a constant aggregate whose
    // f64 slot is a `double 0.0` leaf.
    let good = f64_module_ir(
        "define double @copy(double %a) { ret double %a }\n\
         define {i64, double} @none_rate() { ret {i64, double} {i64 1, double 0.000000e+00} }",
    );
    let report = tv::validate_native_module(
        F64_NATIVE_MODULE,
        &good,
        Options::default(),
        RefinedAdmission::CompositeV12,
    )
    .unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);

    // A 64-bit leaf of the wrong kind passes a width-only check but is
    // the wrong ABI: i64 where `f64` is declared, and `double` where
    // `i64` is declared.
    let int_for_f64 = f64_module_ir(
        "define i64 @copy(i64 %a) { ret i64 %a }\n\
         define {i64, double} @none_rate() { ret {i64, double} {i64 1, double 0.000000e+00} }",
    );
    assert!(tv::validate_native_module(
        F64_NATIVE_MODULE,
        &int_for_f64,
        Options::default(),
        RefinedAdmission::CompositeV12,
    )
    .is_err());

    let i64_module =
        "atom copy(a: i64) -> i64 requires: true; ensures: result == a; body: { a };\n\
         atom main() -> i64 requires: true; ensures: result == 1; body: { copy(1) };";
    let double_for_i64 = f64_module_ir(
        "define double @copy(double %a) { ret double %a }\n\
         define {i64, double} @none_rate() { ret {i64, double} {i64 1, double 0.000000e+00} }",
    );
    assert!(
        tv::validate_native_module(
            i64_module,
            &double_for_i64,
            Options::default(),
            RefinedAdmission::CompositeV12,
        )
        .is_err(),
        "extra none_rate member aside, the i64-source copy must not bind a double formal"
    );

    // Same leaf count, wrong order: `{i64, f64}` source vs
    // `{i64, double}`-swapped aggregate leaf kinds.
    let swapped = f64_module_ir(
        "define double @copy(double %a) { ret double %a }\n\
         define {double, i64} @none_rate() { ret {double, i64} {double 0.000000e+00, i64 1} }",
    );
    assert!(tv::validate_native_module(
        F64_NATIVE_MODULE,
        &swapped,
        Options::default(),
        RefinedAdmission::CompositeV12,
    )
    .is_err());
}

#[test]
fn fast_math_flags_on_admitted_instructions_are_rejected() {
    // The pass-through `f64` leaf's bit pattern must stay opaque — an
    // `nnan`/`fast` flag on a `select` or `call` would let the IR
    // reinterpret it, and LLVM 17's C API offers no getter, so the
    // instruction's printed form is scanned for the closed flag set.
    let module = "atom pick(flag: bool, x: f64, y: f64) -> f64 requires: true; ensures: true; body: { if flag { x } else { y } };\n\
         atom main() -> i64 requires: true; ensures: result == 1; body: { 1 };";
    let pick = |flags: &str| {
        f64_module_ir(&format!(
            "define double @pick(i64 %f, double %x, double %y) {{\n\
             %c = icmp ne i64 %f, 0\n\
             %r = select {flags} i1 %c, double %x, double %y\n\
             ret double %r\n}}"
        ))
    };
    let report = tv::validate_native_module(
        module,
        &pick(""),
        Options::default(),
        RefinedAdmission::CompositeV12,
    )
    .unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    for flags in ["nnan", "fast", "nnan nsz"] {
        assert!(
            tv::validate_native_module(
                module,
                &pick(flags),
                Options::default(),
                RefinedAdmission::CompositeV12,
            )
            .is_err(),
            "select {flags} must reject"
        );
    }
}

/// `opt-o2` snapshot helpers: `default<O2>` canonical output carries a
/// `tail` call marker, a function-scope attribute group and the
/// canonical optimizer intrinsics.
fn opt_attrs() -> &'static str {
    "attributes #0 = { mustprogress nofree norecurse nosync nounwind willreturn memory(none) }\n\
     attributes #1 = { nocallback nofree nosync nounwind speculatable willreturn memory(none) }"
}

#[test]
fn opt_o2_canonical_snapshot_is_equivalent_under_the_pinned_profile() {
    // `if x < y { x } else { y }` is exactly `llvm.smin.i64`.
    let src = source("x: i64, y: i64", "if x < y { x } else { y }");
    let ir = pin(&format!(
        "define i64 @f(i64 %0, i64 %1) local_unnamed_addr #0 {{\n\
         %3 = tail call i64 @llvm.smin.i64(i64 %0, i64 %1)\n\
         ret i64 %3\n\
         }}\n\
         declare i64 @llvm.smin.i64(i64, i64) #1\n\
         {}",
        opt_attrs()
    ));
    let report = tv::validate_pure_i64_optimized(&src, &ir, Options::default()).unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    assert_eq!(report.profile, tv::OPT_PROFILE);
    assert_eq!(
        report.checked_stage,
        "supplied_llvm_ir_snapshot_post_default_O2"
    );
    // The same snapshot stays outside the pre-pipeline profile: the
    // declaration, `tail` marker and attribute group all reject there.
    assert!(tv::validate_pure_i64(&src, &ir, Options::default()).is_err());
}

#[test]
fn opt_o2_scalar_intrinsics_encode_their_source_semantics() {
    // `smax` = signed max; `umin` is the unsigned min — a different
    // function than the signed comparison the source expresses, so a
    // snapshot naming it is a real difference, not an equivalence.
    let smin = source("x: i64, y: i64", "if x < y { x } else { y }");
    let smax = source("x: i64, y: i64", "if x > y { x } else { y }");
    for (src, intrinsic) in [(&smin, "llvm.smin.i64"), (&smax, "llvm.smax.i64")] {
        let ir = pin(&format!(
            "define i64 @f(i64 %0, i64 %1) {{\n\
             %3 = call i64 @{intrinsic}(i64 %0, i64 %1)\n\
             ret i64 %3\n\
             }}\n\
             declare i64 @{intrinsic}(i64, i64)"
        ));
        assert_eq!(
            tv::validate_pure_i64_optimized(src, &ir, Options::default())
                .unwrap()
                .outcome,
            Outcome::EquivalentWithinProfile,
            "{intrinsic}"
        );
    }
    let umin = pin("define i64 @f(i64 %0, i64 %1) {\n\
         %3 = call i64 @llvm.umin.i64(i64 %0, i64 %1)\n\
         ret i64 %3\n\
         }\n\
         declare i64 @llvm.umin.i64(i64, i64)");
    assert_eq!(
        tv::validate_pure_i64_optimized(&smin, &umin, Options::default())
            .unwrap()
            .outcome,
        Outcome::Different
    );
    // `abs` wraps `if x < 0 { 0 - x } else { x }`; with a false
    // `is_int_min_poison` flag the semantics match exactly.
    let abs_src = source("x: i64", "if x < 0 { 0 - x } else { x }");
    let abs_wrapping = pin("define i64 @f(i64 %0) {\n\
         %2 = call i64 @llvm.abs.i64(i64 %0, i1 false)\n\
         ret i64 %2\n\
         }\n\
         declare i64 @llvm.abs.i64(i64, i1)");
    assert_eq!(
        tv::validate_pure_i64_optimized(&abs_src, &abs_wrapping, Options::default())
            .unwrap()
            .outcome,
        Outcome::EquivalentWithinProfile
    );
    // With `is_int_min_poison` the intrinsic poisons i64::MIN where the
    // source wraps — a poisoned-input failure, not a value difference.
    let abs_poison = abs_wrapping.replace("i1 false", "i1 true");
    assert_eq!(
        tv::validate_pure_i64_optimized(&abs_src, &abs_poison, Options::default())
            .unwrap()
            .outcome,
        Outcome::UndefinedBehavior
    );
}

#[test]
fn opt_o2_with_overflow_pair_yields_result_and_flag_leaves() {
    // The wrapped result leaf equals the source's wrapping add; the
    // `i1` flag leaf is readable and binds 1 exactly on overflow.
    let src = source("x: i64, y: i64", "x + y");
    let ir = pin("define i64 @f(i64 %0, i64 %1) {\n\
         %3 = call {i64, i1} @llvm.sadd.with.overflow.i64(i64 %0, i64 %1)\n\
         %4 = extractvalue {i64, i1} %3, 0\n\
         ret i64 %4\n\
         }\n\
         declare {i64, i1} @llvm.sadd.with.overflow.i64(i64, i64)");
    assert_eq!(
        tv::validate_pure_i64_optimized(&src, &ir, Options::default())
            .unwrap()
            .outcome,
        Outcome::EquivalentWithinProfile
    );
    // Feeding the flag into a branch makes a value the source cannot
    // express: equivalent only if the flag encoding is right. `x + y`
    // overflows iff `result < x` when `y >= 0` — the source-side
    // approximation differs on `y < 0` inputs and is a real difference.
    let flagged = ir.replace(
        "%4 = extractvalue {i64, i1} %3, 0\nret i64 %4",
        "%4 = extractvalue {i64, i1} %3, 1\n%5 = select i1 %4, i64 0, i64 1\nret i64 %5",
    );
    let approx = source(
        "x: i64, y: i64",
        "if y >= 0 { if x + y < x { 0 } else { 1 } } else { if x + y > x { 0 } else { 1 } }",
    );
    // The approximation is genuinely different: the flag goes the other
    // way on inputs where the sign of y flips the comparison direction.
    assert_eq!(
        tv::validate_pure_i64_optimized(&approx, &flagged, Options::default())
            .unwrap()
            .outcome,
        Outcome::Different
    );
    // Same source against the `usub` flag: unsigned subtraction
    // underflows exactly when x < y — unsignedness the source cannot
    // say either, and the wrapped subtraction still matches.
    let usub = pin("define i64 @f(i64 %0, i64 %1) {\n\
         %3 = call {i64, i1} @llvm.usub.with.overflow.i64(i64 %0, i64 %1)\n\
         %4 = extractvalue {i64, i1} %3, 0\n\
         ret i64 %4\n\
         }\n\
         declare {i64, i1} @llvm.usub.with.overflow.i64(i64, i64)");
    let sub_src = source("x: i64, y: i64", "x - y");
    assert_eq!(
        tv::validate_pure_i64_optimized(&sub_src, &usub, Options::default())
            .unwrap()
            .outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn opt_o2_freeze_encodes_as_its_operand() {
    let src = source("x: i64", "x + 1");
    let ir = pin("define i64 @f(i64 %0) {\n\
         %2 = freeze i64 %0\n\
         %3 = add i64 %2, 1\n\
         ret i64 %3\n\
         }");
    assert_eq!(
        tv::validate_pure_i64_optimized(&src, &ir, Options::default())
            .unwrap()
            .outcome,
        Outcome::EquivalentWithinProfile
    );
    assert!(tv::validate_pure_i64(&src, &ir, Options::default()).is_err());
}

#[test]
fn opt_o2_keeps_poison_strengthening_forms_rejected() {
    let src = source("x: i64, y: i64", "x | y");
    // `or disjoint`, `nsw`/`nuw`/`exact` and parameter/return
    // attributes constrain the value or the input domain — they stay
    // rejected under every profile.
    for ir in [
        pin("define i64 @f(i64 %0, i64 %1) {\n%3 = or disjoint i64 %0, %1\nret i64 %3\n}"),
        pin("define i64 @f(i64 %0, i64 %1) {\n%3 = or i64 %0, %1\nret i64 %3\n}\nmodule asm \"x\""),
        pin("define noundef i64 @f(i64 %0, i64 %1) {\n%3 = or i64 %0, %1\nret i64 %3\n}"),
        pin("define i64 @f(i64 noundef %0, i64 %1) {\n%3 = or i64 %0, %1\nret i64 %3\n}"),
    ] {
        assert!(
            tv::validate_pure_i64_optimized(&src, &ir, Options::default()).is_err(),
            "{ir}"
        );
    }
}

#[test]
fn opt_o2_module_profile_covers_member_snapshots() {
    let src = "atom f(x: i64) -> i64 requires: true; ensures: true; body: { x + 1 };";
    let ir = pin(&format!(
        "define i64 @f(i64 %0) local_unnamed_addr #0 {{\n\
         %2 = freeze i64 %0\n\
         %3 = add i64 %2, 1\n\
         ret i64 %3\n\
         }}\n\
         {}",
        opt_attrs()
    ));
    let report = tv::validate_pure_i64_module_optimized(src, &ir, Options::default()).unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    assert_eq!(report.profile, tv::OPT_PROFILE);
    assert_eq!(
        report.checked_stage,
        "supplied_llvm_ir_snapshot_post_default_O2"
    );
}

#[test]
fn opt_o2_unsigned_overflow_flags_use_unsigned_predicates() {
    // `uadd`/`usub`/`umul` flags mean unsigned overflow: a
    // signed-predicate encoding would read these inputs as
    // non-overflowing and report the IR different from the source.
    // The comparison domain is unrestricted by `requires`, so each
    // flag is fed a constant bound operand whose overflow corner is
    // expressible in the signed source.
    let uadd = pin("define i64 @f(i64 %0, i64 %1) {\n\
         %3 = call {i64, i1} @llvm.uadd.with.overflow.i64(i64 %0, i64 1)\n\
         %4 = extractvalue {i64, i1} %3, 1\n\
         %5 = select i1 %4, i64 0, i64 1\n\
         ret i64 %5\n\
         }\n\
         declare {i64, i1} @llvm.uadd.with.overflow.i64(i64, i64)");
    // x + 1 wraps unsigned exactly when x is -1 (u64::MAX).
    let add_src = "atom f(x: i64, y: i64) -> i64 requires: true; ensures: true; body: { if x == 0 - 1 { 0 } else { 1 } };";
    assert_eq!(
        tv::validate_pure_i64_optimized(add_src, &uadd, Options::default())
            .unwrap()
            .outcome,
        Outcome::EquivalentWithinProfile
    );
    // x - 1 borrows unsigned exactly when x is 0.
    let usub = uadd.replace("uadd", "usub");
    let sub_src = "atom f(x: i64, y: i64) -> i64 requires: true; ensures: true; body: { if x == 0 { 0 } else { 1 } };";
    assert_eq!(
        tv::validate_pure_i64_optimized(sub_src, &usub, Options::default())
            .unwrap()
            .outcome,
        Outcome::EquivalentWithinProfile
    );
    // 2 * x wraps unsigned exactly when x is negative.
    let umul = pin("define i64 @f(i64 %0, i64 %1) {\n\
         %3 = call {i64, i1} @llvm.umul.with.overflow.i64(i64 %0, i64 2)\n\
         %4 = extractvalue {i64, i1} %3, 1\n\
         %5 = select i1 %4, i64 0, i64 1\n\
         ret i64 %5\n\
         }\n\
         declare {i64, i1} @llvm.umul.with.overflow.i64(i64, i64)");
    let mul_src = "atom f(x: i64, y: i64) -> i64 requires: true; ensures: true; body: { if x < 0 { 0 } else { 1 } };";
    assert_eq!(
        tv::validate_pure_i64_optimized(mul_src, &umul, Options::default())
            .unwrap()
            .outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn opt_o2_function_attributes_are_whitelisted_not_blanketed() {
    let src = source("x: i64", "x");
    // The canonical `default<O2>` function attributes are admitted.
    let canonical = pin(
        "define i64 @f(i64 %x) local_unnamed_addr #0 { ret i64 %x }\n\
         attributes #0 = { mustprogress nofree norecurse nosync nounwind willreturn memory(none) }",
    );
    assert_eq!(
        tv::validate_pure_i64_optimized(&src, &canonical, Options::default())
            .unwrap()
            .outcome,
        Outcome::EquivalentWithinProfile
    );
    // `noreturn` is a semantic contract the comparison does not read:
    // a returning body marked `noreturn` is UB, not an annotation.
    let noreturn = pin("define noreturn i64 @f(i64 %x) { ret i64 %x }");
    assert!(tv::validate_pure_i64_optimized(&src, &noreturn, Options::default()).is_err());
    // An under-claimed `memory` bound is inconsistent: `@f` claims
    // `none` while calling a callee claiming `readwrite`.
    let under = pin("define i64 @f(i64 %x) #0 {\n\
         %r = call i64 @g(i64 %x)\n\
         ret i64 %r\n\
         }\n\
         define i64 @g(i64 %x) #1 { ret i64 %x }\n\
         attributes #0 = { memory(none) }\n\
         attributes #1 = { memory(readwrite) }");
    assert!(tv::validate_pure_i64_optimized(&src, &under, Options::default()).is_err());
}

#[test]
fn opt_o2_freeze_rescues_the_operand_poison_obligation() {
    // `freeze` masks the `abs` i64::MIN poison: the old global
    // obligation reported UndefinedBehavior although the program is
    // defined everywhere. With every use frozen the value becomes
    // arbitrary on the poisoned input, so the comparison still
    // distinguishes it from a source that returns a definite value
    // there — the verdict is Different, not UndefinedBehavior.
    let src = source("x: i64", "if x < 0 { 0 - x } else { x }");
    let frozen = pin("define i64 @f(i64 %x) {\n\
         %v = call i64 @llvm.abs.i64(i64 %x, i1 true)\n\
         %f = freeze i64 %v\n\
         ret i64 %f\n\
         }\n\
         declare i64 @llvm.abs.i64(i64, i1)");
    assert_eq!(
        tv::validate_pure_i64_optimized(&src, &frozen, Options::default())
            .unwrap()
            .outcome,
        Outcome::Different
    );
    // An unfrozen use keeps the obligation: the same function without
    // the freeze still reports UndefinedBehavior on i64::MIN.
    assert_eq!(
        tv::validate_pure_i64_optimized(
            &src,
            &frozen.replace("ret i64 %f", "ret i64 %v"),
            Options::default()
        )
        .unwrap()
        .outcome,
        Outcome::UndefinedBehavior
    );
}

#[test]
fn opt_o2_keeps_the_cancel_poll_call_admitted() {
    // The cancellation poll is a declaration-shaped call that is not
    // an optimizer intrinsic: routing it into the intrinsic table
    // would reject every loop-header snapshot the pipeline emits.
    let ir = pin("define i64 @f(i64 %x) {\n\
         %p = call i64 @__mumei_task_group_should_cancel_current()\n\
         %r = add i64 %x, %p\n\
         ret i64 %r\n\
         }\n\
         declare i64 @__mumei_task_group_should_cancel_current()");
    let src = source("x: i64", "x");
    assert_eq!(
        tv::validate_pure_i64_optimized(&src, &ir, Options::default())
            .unwrap()
            .outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn opt_o2_reads_constant_overflow_pairs() {
    // A literal `{i64, i1}` pair constant reaches the extractor through
    // the same leaf layout a computed pair does.
    let ir = pin("define i64 @f(i64 %x) {\n\
         %v = extractvalue {i64, i1} zeroinitializer, 0\n\
         ret i64 %v\n\
         }");
    assert_eq!(
        tv::validate_pure_i64_optimized(&source("x: i64", "0"), &ir, Options::default())
            .unwrap()
            .outcome,
        Outcome::EquivalentWithinProfile
    );
    // A non-opt profile still refuses the pair constant.
    assert!(tv::validate_pure_i64(&source("x: i64", "0"), &ir, Options::default()).is_err());
}

#[test]
fn opt_o2_division_is_immediate_ub_not_freeze_rescuable() {
    // `sdiv`/`udiv` by zero is immediate UB in LLVM, not poison: a
    // `freeze` of the quotient must not rescue the obligation the way
    // it rescues a poisoned `abs`. Tagging division obligations as
    // rescuable would hide the UB and report a mere value difference.
    // The quotient is frozen then cancelled to a constant, so both
    // sides return `0` on every defined input and only the zero-divisor
    // obligation separates them: tagging division as rescuable would
    // let the `freeze` mask it and report a vacuous equivalence.
    for op in ["sdiv", "udiv", "srem", "urem"] {
        let ir = pin(&format!(
            "define i64 @f(i64 %x, i64 %y) {{\n\
             %q = {op} i64 %x, %y\n\
             %f = freeze i64 %q\n\
             %r = sub i64 %f, %f\n\
             ret i64 %r\n\
             }}"
        ));
        assert_eq!(
            tv::validate_pure_i64_optimized(
                &source("x: i64, y: i64", "0"),
                &ir,
                Options::default()
            )
            .unwrap()
            .outcome,
            Outcome::UndefinedBehavior,
            "{op} by a possibly-zero operand is immediate UB"
        );
    }
    // A definite divisor still validates normally: `sdiv x, 1` is the
    // identity and its `freeze` encodes as the operand.
    let identity = pin("define i64 @f(i64 %x, i64 %y) {\n\
         %q = sdiv i64 %x, 1\n\
         %f = freeze i64 %q\n\
         ret i64 %f\n\
         }");
    assert_eq!(
        tv::validate_pure_i64_optimized(
            &source("x: i64, y: i64", "x"),
            &identity,
            Options::default()
        )
        .unwrap()
        .outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn opt_o2_freeze_rescue_requires_every_operand_use_frozen() {
    // Poison escapes a rescue when the operand is also consumed by a
    // non-freeze instruction: `%v` feeds both the `freeze` and an
    // ordinary `add`. The derived use is dead here, but the checker
    // fails closed — an unfrozen use of a poisoned operand is not
    // provably harmless, so the obligation stays and i64::MIN reports
    // UndefinedBehavior instead of being hidden by the rescue.
    let src = source("x: i64", "if x < 0 { 0 - x } else { x }");
    let leaky = pin("define i64 @f(i64 %x) {\n\
         %v = call i64 @llvm.abs.i64(i64 %x, i1 true)\n\
         %a = add i64 %v, 1\n\
         %f = freeze i64 %v\n\
         ret i64 %f\n\
         }\n\
         declare i64 @llvm.abs.i64(i64, i1)");
    assert_eq!(
        tv::validate_pure_i64_optimized(&src, &leaky, Options::default())
            .unwrap()
            .outcome,
        Outcome::UndefinedBehavior
    );
}

#[test]
fn opt_o2_freeze_of_an_i1_flag_uses_the_operand_width() {
    // The rescue's fresh value must carry the operand's bit width: a
    // 64-bit constant against an `i1` operand sorts-mismatches Z3's
    // `ite` and fails the whole check instead of validating.
    let ir = pin("define i64 @f(i64 %x, i64 %y) {\n\
         %p = call {i64, i1} @llvm.uadd.with.overflow.i64(i64 %x, i64 1)\n\
         %flag = extractvalue {i64, i1} %p, 1\n\
         %f = freeze i1 %flag\n\
         %r = select i1 %f, i64 0, i64 1\n\
         ret i64 %r\n\
         }\n\
         declare {i64, i1} @llvm.uadd.with.overflow.i64(i64, i64)");
    // x + 1 wraps unsigned exactly when x is -1 (u64::MAX).
    let src = "atom f(x: i64, y: i64) -> i64 requires: true; ensures: true; body: { if x == 0 - 1 { 0 } else { 1 } };";
    assert_eq!(
        tv::validate_pure_i64_optimized(src, &ir, Options::default())
            .unwrap()
            .outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn opt_o2_module_absent_callee_memory_claim_is_unrestricted() {
    // A callee without a `memory` attribute may touch anything, so
    // its implicit claim is `memory(readwrite)`, not a superset of
    // every encodable bit. A caller claiming `readwrite` is exactly
    // consistent with that and must validate — the previous `u64::MAX`
    // fallback made every real caller under-claim.
    let ir = pin("define i64 @g(i64 %x) { ret i64 %x }\n\
         define i64 @f(i64 %y) #0 {\n\
           %a = call i64 @g(i64 %y)\n\
           %b = add i64 %y, 1\n\
           %c = call i64 @g(i64 %b)\n\
           %d = add i64 %a, %c\n\
           ret i64 %d\n\
         }\n\
         attributes #0 = { memory(readwrite) }");
    let report = tv::validate_pure_i64_module_optimized(CALLING, &ir, Options::default()).unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
}

const ENUM_EQUALITY_SOURCE: &str = "enum Chan { Tcp(p: i64), Idle }\nenum Tag { Some(b: bool), None }\natom is_idle(c: Chan) -> bool requires: true; ensures: true; body: { c == Chan::Idle };\natom differs(a: Chan, b: Chan) -> bool requires: true; ensures: true; body: { a != b };\natom same_tag(t: Tag) -> bool requires: true; ensures: true; body: { t == Tag::None };\natom main() -> i64 requires: true; ensures: true; body: { 0 };";

#[test]
fn composite_v14_enum_equality_compares_the_whole_tagged_union_layout() {
    // The emitter lowers `e == e'` to an `extractvalue`/`icmp` chain
    // over the tag and every payload slot (padding included), anded —
    // the source model's conjuncts must track that exact layout.
    let items = parse_module_checked(ENUM_EQUALITY_SOURCE).unwrap();
    let ir = emitted_module(&items);
    assert!(
        ir.contains("icmp eq") && ir.contains("extractvalue"),
        "emitter did not lower enum equality positionally:\n{ir}"
    );
    let report = tv::validate_native_module(
        ENUM_EQUALITY_SOURCE,
        &ir,
        Options::default(),
        RefinedAdmission::CompositeV14,
    )
    .unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);

    // Comparing only the tag and skipping the payload slot is a wrong
    // result under `Tcp(x) == Tcp(y)` — must refute.
    let tag_only = ir.replacen(
        "%enum_eq_l1 = extractvalue { i64, i64 } %0, 1",
        "%enum_eq_l1 = extractvalue { i64, i64 } %0, 0",
        1,
    );
    assert_ne!(tag_only, ir, "probe did not apply");
    assert_eq!(
        tv::validate_native_module(
            ENUM_EQUALITY_SOURCE,
            &tag_only,
            Options::default(),
            RefinedAdmission::CompositeV14,
        )
        .unwrap()
        .outcome,
        Outcome::Different
    );
}

#[test]
fn composite_v14_enum_equality_is_a_new_widening_not_a_relaxation() {
    // Below v14 the enum-equality source itself is refused.
    assert!(tv::validate_native_module(
        ENUM_EQUALITY_SOURCE,
        "define i64 @main() { ret i64 0 }",
        Options::default(),
        RefinedAdmission::CompositeV13,
    )
    .is_err());

    // Equality across two different enum types stays rejected at v14.
    let mixed = "enum Chan { Tcp(p: i64), Idle }\nenum Mode { On, Off }\natom bad(c: Chan, m: Mode) -> bool requires: true; ensures: true; body: { c == m };\natom main() -> i64 requires: true; ensures: true; body: { 0 };";
    assert!(tv::validate_native_module(
        mixed,
        "define i64 @main() { ret i64 0 }",
        Options::default(),
        RefinedAdmission::CompositeV14,
    )
    .is_err());

    // `.tag` reads have no emitter lowering — they stay out of the
    // fragment even at v14.
    let tag_read = "enum Chan { Tcp(p: i64), Idle }\natom tag_of(c: Chan) -> i64 requires: true; ensures: true; body: { c.tag };\natom main() -> i64 requires: true; ensures: true; body: { 0 };";
    assert!(tv::validate_native_module(
        tag_read,
        "define i64 @main() { ret i64 0 }",
        Options::default(),
        RefinedAdmission::CompositeV14,
    )
    .is_err());
}

#[test]
fn composite_v14_match_produced_enum_equality_validates() {
    // A `match` that produces enum values in every arm types as the
    // enum — the emitter resolves the declared type for `==` and the
    // model merges the arms slot-wise. The source model requires a
    // catch-all arm, same as the emitter's unreachable fallback.
    let src = "enum Chan { Tcp(p: i64), Idle }\natom pick(c: Chan, a: Chan) -> bool requires: true; ensures: true; body: { match c { Chan::Tcp(p) => a, _ => Chan::Idle } == a };\natom main() -> i64 requires: true; ensures: true; body: { 0 };";
    let items = parse_module_checked(src).unwrap();
    let ir = emitted_module(&items);
    assert!(ir.contains("enum_eq"), "no enum eq emitted:\n{ir}");
    let report =
        tv::validate_native_module(src, &ir, Options::default(), RefinedAdmission::CompositeV14)
            .unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    // The let-bound form takes the same phi + `undef` fallback arm on
    // the IR side and the same per-slot merge on the source side.
    let let_src = "enum Chan { Tcp(p: i64), Idle }\natom pick(c: Chan, a: Chan) -> bool requires: true; ensures: true; body: { let m = match c { Chan::Tcp(p) => a, _ => Chan::Idle, }; m == a };\natom main() -> i64 requires: true; ensures: true; body: { 0 };";
    let let_items = parse_module_checked(let_src).unwrap();
    let let_ir = emitted_module(&let_items);
    let report = tv::validate_native_module(
        let_src,
        &let_ir,
        Options::default(),
        RefinedAdmission::CompositeV14,
    )
    .unwrap();
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
}

#[test]
fn composite_v14_f64_payload_enum_equality_is_refused() {
    // The emitter compares f64 slots with `fcmp oeq`, which bitwise
    // equality cannot model — fail closed until the model encodes it.
    let src = "enum Rate { Fixed(v: f64), None }\natom same(a: Rate, b: Rate) -> bool requires: true; ensures: true; body: { a == b };\natom main() -> i64 requires: true; ensures: true; body: { 0 };";
    assert!(tv::validate_native_module(
        src,
        "define i64 @main() { ret i64 0 }",
        Options::default(),
        RefinedAdmission::CompositeV14,
    )
    .is_err());
}

#[test]
fn composite_v23_does_not_admit_deep_enum_payload_writes() {
    let baseline = "struct Inner { x: i64, y: i64 }\n\
        struct Outer { inner: Inner, z: i64 }\n\
        enum Event { Move(p: Outer), Idle }\n\
        atom main() -> i64 requires: true; ensures: true; body: {\n\
            let e = Event::Move(Outer { inner: Inner { x: 1, y: 2 }, z: 3 });\n\
            0\n\
        };";
    let baseline_items = parse_module_checked(baseline).unwrap();
    let baseline_ir = emitted_module(&baseline_items);
    let baseline_report = tv::validate_native_module(
        baseline,
        &baseline_ir,
        Options::default(),
        RefinedAdmission::CompositeV23,
    )
    .unwrap();
    assert_eq!(baseline_report.outcome, Outcome::EquivalentWithinProfile);

    let deep_write = baseline.replace("0\n", "e.p.z = 7;\n0\n");
    let deep_write_items = parse_module_checked(&deep_write).unwrap();
    let deep_write_ir = emitted_module(&deep_write_items);
    assert!(
        tv::validate_native_module(
            &deep_write,
            &deep_write_ir,
            Options::default(),
            RefinedAdmission::CompositeV23,
        )
        .is_err(),
        "deep enum payload writes must remain outside composite TV admission"
    );
}

#[test]
fn composite_v23_does_not_admit_enum_slot_deep_writes() {
    let source = "enum Chan { Tcp(port: i64), Udp }\n\
        struct B { e: Chan, k: i64 }\n\
        atom main() -> i64 requires: true; ensures: true; body: {\n\
            let b = B { e: Chan::Tcp(1), k: 3 };\n\
            b.e.port = 7;\n\
            0\n\
        };";
    let items = parse_module_checked(source).unwrap();
    let ir = emitted_module(&items);
    assert!(
        tv::validate_native_module(
            source,
            &ir,
            Options::default(),
            RefinedAdmission::CompositeV23,
        )
        .is_err(),
        "enum-slot deep writes must remain outside composite TV admission"
    );
}

#[test]
fn composite_v23_does_not_admit_direct_enum_payload_reads() {
    let source = "enum Chan { Tcp(port: i64, keep: i64), Udp }\n\
        atom main() -> i64 requires: true; ensures: true; body: {\n\
            let e = Chan::Tcp(5, 9);\n\
            e.port\n\
        };";
    let items = parse_module_checked(source).unwrap();
    let ir = emitted_module(&items);
    assert!(
        tv::validate_native_module(
            source,
            &ir,
            Options::default(),
            RefinedAdmission::CompositeV23,
        )
        .is_err(),
        "direct enum payload reads must not expand CompositeV23 admission"
    );
}

#[test]
fn composite_v23_does_not_admit_struct_valued_enum_payload_reads() {
    let source = "struct Inner { x: i64, y: i64 }\n\
        struct P { inner: Inner, z: i64 }\n\
        enum Ev { Move(p: P), Stop }\n\
        atom main() -> i64 requires: true; ensures: true; body: {\n\
            let e = Ev::Move(P { inner: Inner { x: 1, y: 2 }, z: 3 });\n\
            let q = e.p;\n\
            q.inner.x\n\
        };";
    let items = parse_module_checked(source).unwrap();
    let ir = emitted_module(&items);
    assert!(
        tv::validate_native_module(
            source,
            &ir,
            Options::default(),
            RefinedAdmission::CompositeV23,
        )
        .is_err(),
        "struct-valued direct enum payload reads must not expand CompositeV23 admission"
    );
}

// ─── hir → llvm_ir correspondence (scalar-i64 leaf subset) ────────────

fn hir_outcome(source: &str, ir: &str) -> Outcome {
    let Item::Atom(atom) = parse_module_checked(source).unwrap().remove(0) else {
        panic!("atom")
    };
    let hir = lower_atom_to_hir(&atom);
    tv::validate_hir_pure_i64_leaf(
        &hir,
        &BTreeMap::new(),
        &BTreeMap::new(),
        ir,
        false,
        Options::default(),
    )
    .unwrap()
}

#[test]
fn hir_leaf_compare_is_equivalent_on_emitted_ir() {
    let src = source("x: i64, y: i64", "x * y + 1");
    assert_eq!(
        hir_outcome(&src, &emitted(&src)),
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn hir_leaf_compare_keeps_plain_ir_main_names() {
    // `native_main_rename = false` compares call targets as written:
    // `main()` meets `@main`, and `@__mumei_user_main` — the bundle
    // emitter's private name — is simply a different function. The
    // rename stays the recheck path's alone.
    let src = "atom caller() -> i64 requires: true; ensures: true; body: { main() };\n\
               atom main() -> i64 requires: true; ensures: true; body: { 7 };";
    let plain = pin(
        "define i64 @caller() {\n %r = call i64 @main()\n ret i64 %r }\n\
         define i64 @main() { ret i64 7 }",
    );
    assert_eq!(hir_outcome(src, &plain), Outcome::EquivalentWithinProfile);
    let renamed = plain.replace("@main", "@__mumei_user_main");
    assert_eq!(hir_outcome(src, &renamed), Outcome::Different);
}

#[test]
fn hir_leaf_compare_catches_a_mistranslated_ir() {
    // The machine side cannot flag a faithfully-lowered wrong IR; the
    // HIR-anchored check compares the HIR model against the IR itself.
    let ir = pin("define i64 @f(i64 %x, i64 %y) { %s = sub i64 %x, %y\n ret i64 %s }");
    assert_eq!(
        hir_outcome(&source("x: i64, y: i64", "x + y"), &ir),
        Outcome::Different
    );
}

#[test]
fn hir_leaf_compare_binds_definedness_both_ways() {
    // A missing UB guard on the IR side is UndefinedBehavior, not a
    // value mismatch — the same direction the source check enforces.
    let src = source("x: i64, y: i64", "x / y");
    assert_eq!(
        hir_outcome(&src, &emitted(&src)),
        Outcome::EquivalentWithinProfile
    );
    let ir = pin("define i64 @f(i64 %x, i64 %y) { %q = sdiv i64 %x, 0\n ret i64 %q }");
    assert_eq!(hir_outcome(&src, &ir), Outcome::UndefinedBehavior);
}

#[test]
fn hir_leaf_compare_rejects_atoms_outside_the_subset() {
    // A non-i64 parameter is outside the leaf subset — fail closed, no
    // verdict.
    let Item::Atom(atom) = parse_module_checked(&source("x: f64", "1"))
        .unwrap()
        .remove(0)
    else {
        panic!("atom")
    };
    let hir = lower_atom_to_hir(&atom);
    assert!(tv::validate_hir_pure_i64_leaf(
        &hir,
        &BTreeMap::new(),
        &BTreeMap::new(),
        &pin("define i64 @f(i64 %x) { ret i64 %x }"),
        false,
        Options::default()
    )
    .is_err());
}

#[test]
fn hir_leaf_compare_binds_scalar_i64_call_sites() {
    // f calls g: both sides produce `mumei_call_g_1` on the same arg —
    // the call compares as an uninterpreted site, so a wrong IR call
    // (to h, or with swapped args) is caught.
    let src = source("x: i64", "g(x + 1)");
    let ir = pin(
        "define i64 @g(i64 %a) { %r = add i64 %a, 0\n ret i64 %r }\n\
         define i64 @f(i64 %x) { %v = add i64 %x, 1\n %r = call i64 @g(i64 %v)\n ret i64 %r }",
    );
    assert_eq!(hir_outcome(&src, &ir), Outcome::EquivalentWithinProfile);

    // Same shape, different callee — the site names differ.
    let wrong = pin(
        "define i64 @h(i64 %a) { %r = add i64 %a, 0\n ret i64 %r }\n\
         define i64 @g(i64 %a) { %r = add i64 %a, 0\n ret i64 %r }\n\
         define i64 @f(i64 %x) { %v = add i64 %x, 1\n %r = call i64 @h(i64 %v)\n ret i64 %r }",
    );
    assert_eq!(hir_outcome(&src, &wrong), Outcome::Different);

    // Same callee, different argument.
    let wrong_arg = pin(
        "define i64 @g(i64 %a) { %r = add i64 %a, 0\n ret i64 %r }\n\
         define i64 @f(i64 %x) { %v = add i64 %x, 2\n %r = call i64 @g(i64 %v)\n ret i64 %r }",
    );
    assert_eq!(hir_outcome(&src, &wrong_arg), Outcome::Different);
}

#[test]
fn hir_leaf_compare_guards_branch_definedness() {
    // `x / y` only in the else arm: y = 0 is in-domain whenever
    // x > 0 (then-branch taken), so the check stays equivalent.
    let src = source("x: i64, y: i64", "if x > 0 { 1 } else { x / y }");
    assert_eq!(
        hir_outcome(&src, &emitted(&src)),
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn hir_leaf_compare_short_circuits_rhs_definedness() {
    // `c` may be 0 whenever `a == 0`: `&&` never runs the rhs there, so
    // the comparison cannot bind `c != 0` on those inputs.
    let src = source("a: i64, b: i64, c: i64", "a != 0 && (b / c) > 0");
    assert_eq!(
        hir_outcome(&src, &emitted(&src)),
        Outcome::EquivalentWithinProfile
    );
}

fn source_ret(params: &str, ret: &str, body: &str) -> String {
    format!("atom f({params}) -> {ret} requires: true; ensures: true; body: {{ {body} }};")
}

#[test]
fn hir_leaf_compare_uniform_i128_signatures() {
    let src = source_ret("x: i128, y: i128", "i128", "x + y * 2");
    assert_eq!(
        hir_outcome(&src, &emitted(&src)),
        Outcome::EquivalentWithinProfile
    );
    // Upper-64 divergence only: truncation hides it from an i64 view but
    // not from the 128-bit model.
    let ir = pin(
        "define i128 @f(i128 %x, i128 %y) { %m = mul i128 %y, 3\n %r = add i128 %x, %m\n ret i128 %r }",
    );
    assert_eq!(hir_outcome(&src, &ir), Outcome::Different);
}

#[test]
fn hir_leaf_compare_uniform_u128_uses_unsigned_ops() {
    let src = source_ret("x: u128, y: u128", "u128", "x > y");
    assert_eq!(
        hir_outcome(&src, &emitted(&src)),
        Outcome::EquivalentWithinProfile
    );
    // A signed comparison on the same bits is a mistranslation.
    let ir = pin("define i128 @f(i128 %x, i128 %y) { %c = icmp sgt i128 %x, %y\n %r = select i1 %c, i128 1, i128 0\n ret i128 %r }");
    assert_eq!(hir_outcome(&src, &ir), Outcome::Different);
}

#[test]
fn hir_leaf_compare_rejects_mixed_width_signatures() {
    // i64 param + i128 result mixes widths — outside this slice.
    let src = source_ret("x: i64", "i128", "x");
    let Item::Atom(atom) = parse_module_checked(&src).unwrap().remove(0) else {
        panic!("atom")
    };
    let hir = lower_atom_to_hir(&atom);
    let ir = pin("define i64 @f(i64 %x) { ret i64 %x }");
    assert!(
        tv::validate_hir_pure_i64_leaf(
            &hir,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &ir,
            false,
            Options::default()
        )
        .is_err(),
        "mixed-width signatures stay outside the subset"
    );
}

#[test]
fn hir_leaf_compare_uniform_f64_pass_through() {
    let src = source_ret("x: f64, y: f64", "f64", "let z = x; z");
    assert_eq!(
        hir_outcome(&src, &emitted(&src)),
        Outcome::EquivalentWithinProfile
    );
    // Returning the other parameter is a mistranslation — the model
    // binds the IEEE-754 bit pattern, so it shows up as Different.
    let ir = pin("define double @f(double %x, double %y) { ret double %y }");
    assert_eq!(hir_outcome(&src, &ir), Outcome::Different);
}

#[test]
fn hir_leaf_compare_rejects_f64_operations_and_wrong_ret() {
    // FP arithmetic stays outside the pass-through subset.
    let src = source_ret("x: f64, y: f64", "f64", "x + y");
    let Item::Atom(atom) = parse_module_checked(&src).unwrap().remove(0) else {
        panic!("atom")
    };
    let hir = lower_atom_to_hir(&atom);
    let ir = pin("define double @f(double %x, double %y) { ret double %x }");
    assert!(
        tv::validate_hir_pure_i64_leaf(
            &hir,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &ir,
            false,
            Options::default()
        )
        .is_err(),
        "f64 arithmetic stays outside the pass-through subset"
    );
    // An `i64` return where the source declares `f64` fails the
    // declared-kind gate rather than comparing the bit pattern.
    let src = source_ret("x: f64", "f64", "x");
    let Item::Atom(atom) = parse_module_checked(&src).unwrap().remove(0) else {
        panic!("atom")
    };
    let hir = lower_atom_to_hir(&atom);
    // `ret i64 0` is fully inside the instruction subset — only the
    // declared-kind check on the return rejects it; without that gate
    // this would compare `Different` instead of erroring.
    let ir = pin("define i64 @f(double %x) { ret i64 0 }");
    assert!(
        tv::validate_hir_pure_i64_leaf(
            &hir,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &ir,
            false,
            Options::default()
        )
        .is_err(),
        "an i64 return where f64 is declared fails the kind gate"
    );
}

#[test]
fn hir_leaf_compare_i128_upper_half_divergence() {
    let src = source_ret("x: i128", "i128", "x");
    // Only the upper half differs: bit 64 set — invisible below 2^64.
    let ir = pin("define i128 @f(i128 %x) { %r = or i128 %x, 18446744073709551616\n ret i128 %r }");
    assert_eq!(hir_outcome(&src, &ir), Outcome::Different);
}

#[test]
fn hir_leaf_compare_u128_negative_literal_zero_extends() {
    // `x + -1` in u128: the emitter zero-extends the 64-bit literal, so
    // this is x + (2^64-1), not x - 1 — a sext model would misjudge both.
    let src = source_ret("x: u128", "u128", "x + -1");
    assert_eq!(
        hir_outcome(&src, &emitted(&src)),
        Outcome::EquivalentWithinProfile
    );
    let ir = pin("define i128 @f(i128 %x) { %r = add i128 %x, -1\n ret i128 %r }");
    assert_eq!(hir_outcome(&src, &ir), Outcome::Different);
}

#[test]
fn hir_leaf_compare_rejects_ordered_ops_over_call_results() {
    // `g` may be a u64-returning callee — ordered ops over its result
    // need the callee signature env, so they fail closed for now.
    let src = source("x: i64, y: i64", "g(x) > y");
    let Item::Atom(atom) = parse_module_checked(&src).unwrap().remove(0) else {
        panic!("atom")
    };
    let hir = lower_atom_to_hir(&atom);
    let ir = pin("declare i64 @g(i64)\ndefine i64 @f(i64 %x, i64 %y) { %c = call i64 @g(i64 %x)\n %cmp = icmp sgt i64 %c, %y\n %r = zext i1 %cmp to i64\n ret i64 %r }");
    assert!(
        tv::validate_hir_pure_i64_leaf(
            &hir,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &ir,
            false,
            Options::default()
        )
        .is_err(),
        "ordered ops over a call result stay outside the subset"
    );
}

#[test]
fn hir_leaf_compare_handles_bounded_array_reads() {
    // `a[i]` is the emitter's bounds-check + load + merged `phi(loaded, 0)`
    // — an out-of-bounds read yields 0 on both sides.
    let src = source("a: [i64], i: i64", "a[i]");
    assert_eq!(
        hir_outcome(&src, &emitted(&src)),
        Outcome::EquivalentWithinProfile
    );
    // The same read on the wrong element is a genuine mismatch.
    let wrong = emitted(&source("a: [i64], i: i64", "a[i] + 1"));
    assert_eq!(hir_outcome(&src, &wrong), Outcome::Different);
}

#[test]
fn hir_leaf_compare_binds_array_length() {
    // `len(a)` reads the stored element count — the `arrlen` slot.
    let src = source("a: [i64]", "len(a)");
    assert_eq!(
        hir_outcome(&src, &emitted(&src)),
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn hir_leaf_compare_rejects_array_writes_and_whole_values() {
    // `[i64]` parameters are read-only in this profile: a store stays
    // outside the leaf subset, and so does passing `a` itself as a value.
    // A real IR module: if the HIR side wrongly admitted the body, the
    // comparison would produce a verdict rather than the expected
    // rejection — an empty module could not tell the two apart.
    let real_ir = emitted(&source("a: [i64], i: i64", "a[i]"));
    for body in ["a[0] = 1; a[i]", "g(a)"] {
        let src = source("a: [i64], i: i64", body);
        let Item::Atom(atom) = parse_module_checked(&src).unwrap().remove(0) else {
            panic!("atom")
        };
        let hir = lower_atom_to_hir(&atom);
        assert!(
            tv::validate_hir_pure_i64_leaf(
                &hir,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &real_ir,
                false,
                Options::default()
            )
            .is_err(),
            "{body} stays outside the read-only leaf subset"
        );
    }
}

fn hir_atom(source: &str) -> mumei_core::hir::HirAtom {
    let items = parse_module_checked(source).unwrap();
    let Some(Item::Atom(atom)) = items.into_iter().find(|i| matches!(i, Item::Atom(_))) else {
        panic!("atom")
    };
    lower_atom_to_hir(&atom)
}

/// The module's callee signatures for `encode_hir_pure_i64_leaf` —
/// the same map `hir_llvm_atom_outcomes` derives from `env.atoms` on
/// the recheck path.
fn call_sigs(source: &str) -> BTreeMap<String, mumei_core::semantic_contract::HirCallLeafSig> {
    let items = parse_module_checked(source).unwrap();
    // Mirror the recheck's env resolution: refined aliases fold to
    // their machine base (`Pos` → `i64`) the way the IR operand does.
    let mut env = mumei_core::verification::ModuleEnv::default();
    for item in &items {
        match item {
            Item::TypeDef(typedef) => env.register_type(typedef),
            Item::StructDef(def) => env.register_struct(def),
            Item::EnumDef(def) => env.register_enum(def),
            Item::Atom(atom) => env.register_atom(atom),
            _ => {}
        }
    }
    items
        .iter()
        .filter_map(|item| match item {
            Item::Atom(atom) => Some((
                atom.name.clone(),
                mumei_core::semantic_contract::HirCallLeafSig {
                    params: atom
                        .params
                        .iter()
                        .map(|param| {
                            param
                                .type_name
                                .as_deref()
                                .map(|ty| env.resolve_base_type(ty))
                        })
                        .collect(),
                    result: atom
                        .return_type
                        .as_deref()
                        .map(|ty| env.resolve_base_type(ty)),
                },
            )),
            _ => None,
        })
        .collect()
}

fn hir_outcome_with_structs(
    source: &str,
    structs: &BTreeMap<String, Vec<(String, String)>>,
    ir: &str,
) -> Outcome {
    tv::validate_hir_pure_i64_leaf(
        &hir_atom(source),
        structs,
        &call_sigs(source),
        ir,
        false,
        Options::default(),
    )
    .unwrap()
}

/// `emitted` for sources whose first item is a struct declaration:
/// pick the atom out of the item list before lowering, and register
/// the struct definitions so `p.x`/`Pair{..}` resolve at codegen.
fn emitted_atom(source: &str) -> String {
    let items = parse_module_checked(source).unwrap();
    let mut env = ModuleEnv::new();
    for item in &items {
        match item {
            Item::StructDef(def) => env.register_struct(def),
            Item::Atom(atom) => env.register_atom(atom),
            _ => {}
        }
    }
    pin(
        &mumei_emit_llvm::codegen::compile_to_module(&hir_atom(source).emit_view(), &env, &[])
            .unwrap(),
    )
}

fn pair_env() -> BTreeMap<String, Vec<(String, String)>> {
    BTreeMap::from([(
        "Pair".to_string(),
        vec![
            ("x".to_string(), "i64".to_string()),
            ("y".to_string(), "i64".to_string()),
        ],
    )])
}

fn pair_source(params: &str, result: &str, body: &str) -> String {
    format!(
        "struct Pair {{ x: i64, y: i64 }}\natom f({params}) -> {result} requires: true; ensures: true; body: {{ {body} }};"
    )
}

#[test]
fn hir_leaf_compare_handles_aggregate_params() {
    // `p: Pair` flattens to `{i64, i64}` — `p.x` binds the first slot.
    let src = pair_source("p: Pair", "i64", "p.x");
    assert_eq!(
        hir_outcome_with_structs(&src, &pair_env(), &emitted_atom(&src)),
        Outcome::EquivalentWithinProfile
    );
    // Reading the second leaf instead is a genuine mismatch.
    let ir =
        pin("define i64 @f({i64, i64} %p) { %x = extractvalue {i64, i64} %p, 1\n ret i64 %x }");
    assert_eq!(
        hir_outcome_with_structs(&src, &pair_env(), &ir),
        Outcome::Different
    );
}

#[test]
fn hir_leaf_compare_handles_aggregate_results() {
    // A `Pair` result returns as `{i64, i64}` — `aggregate_value`'s
    // leaf-first packing on both sides.
    let src = pair_source("a: i64, b: i64", "Pair", "Pair{x: a, y: b}");
    assert_eq!(
        hir_outcome_with_structs(&src, &pair_env(), &emitted_atom(&src)),
        Outcome::EquivalentWithinProfile
    );
    // Field-init order follows declaration order, not written order —
    // `Pair{y: b, x: a}` is the same value.
    let swapped = pair_source("a: i64, b: i64", "Pair", "Pair{y: b, x: a}");
    assert_eq!(
        hir_outcome_with_structs(&swapped, &pair_env(), &emitted_atom(&src)),
        Outcome::EquivalentWithinProfile
    );
    let ir = pin("define {i64, i64} @f(i64 %a, i64 %b) { %p0 = insertvalue {i64, i64} undef, i64 %b, 0\n %p1 = insertvalue {i64, i64} %p0, i64 %a, 1\n ret {i64, i64} %p1 }");
    assert_eq!(
        hir_outcome_with_structs(&src, &pair_env(), &ir),
        Outcome::Different
    );
}

#[test]
fn hir_leaf_compare_models_composite_call_sites() {
    // A struct-returning call applies one `mumei_call_field`
    // uninterpreted function per leaf, and a composite argument
    // splices its leaf slots — the same terms the IR `call` encoding
    // produces at the site, so a `sum(mk(1, 2))` chain compares
    // equivalent instead of diverging on mismatched arities.
    let src = "struct Pair { x: i64, y: i64 }\n\
               atom caller() -> i64 requires: true; ensures: true; body: { sum(mk(1, 2)) };\n\
               atom sum(p: Pair) -> i64 requires: true; ensures: true; body: { p.x + p.y };\n\
               atom mk(a: i64, b: i64) -> Pair requires: true; ensures: true; body: { Pair { x: a, y: b } };";
    let ir = pin(
        "define {i64, i64} @mk(i64 %a, i64 %b) {\n  %p0 = insertvalue {i64, i64} undef, i64 %a, 0\n  %p = insertvalue {i64, i64} %p0, i64 %b, 1\n  ret {i64, i64} %p\n}\n\
         define i64 @sum({i64, i64} %p) {\n  %x = extractvalue {i64, i64} %p, 0\n  %y = extractvalue {i64, i64} %p, 1\n  %r = add i64 %x, %y\n  ret i64 %r\n}\n\
         define i64 @caller() {\n  %p = call {i64, i64} @mk(i64 1, i64 2)\n  %r = call i64 @sum({i64, i64} %p)\n  ret i64 %r\n}",
    );
    assert_eq!(
        hir_outcome_with_structs(src, &pair_env(), &ir),
        Outcome::EquivalentWithinProfile
    );
    // Reordering the callee's leaf arguments is a genuine mismatch.
    let swapped = pin(
        "define {i64, i64} @mk(i64 %a, i64 %b) {\n  %p0 = insertvalue {i64, i64} undef, i64 %a, 0\n  %p = insertvalue {i64, i64} %p0, i64 %b, 1\n  ret {i64, i64} %p\n}\n\
         define i64 @sum({i64, i64} %p) {\n  %x = extractvalue {i64, i64} %p, 0\n  %y = extractvalue {i64, i64} %p, 1\n  %r = add i64 %x, %y\n  ret i64 %r\n}\n\
         define i64 @caller() {\n  %p = call {i64, i64} @mk(i64 2, i64 1)\n  %r = call i64 @sum({i64, i64} %p)\n  ret i64 %r\n}",
    );
    assert_eq!(
        hir_outcome_with_structs(src, &pair_env(), &swapped),
        Outcome::Different
    );
}

#[test]
fn hir_leaf_compare_flattens_composite_call_arguments() {
    // `sum(p)` passes the parameter's leaf slots — `sum_2(p.x, p.y)`
    // on both sides — so a struct-parameter pass-through call site
    // stays inside the leaf subset.
    let src = "struct Pair { x: i64, y: i64 }\n\
               atom fwd(p: Pair) -> i64 requires: true; ensures: true; body: { sum(p) };\n\
               atom sum(p: Pair) -> i64 requires: true; ensures: true; body: { p.x + p.y };";
    let ir = pin(
        "define i64 @sum({i64, i64} %p) {\n  %x = extractvalue {i64, i64} %p, 0\n  %y = extractvalue {i64, i64} %p, 1\n  %r = add i64 %x, %y\n  ret i64 %r\n}\n\
         define i64 @fwd({i64, i64} %p) {\n  %r = call i64 @sum({i64, i64} %p)\n  ret i64 %r\n}",
    );
    assert_eq!(
        hir_outcome_with_structs(src, &pair_env(), &ir),
        Outcome::EquivalentWithinProfile
    );
    // Swapped leaf order at the site is a genuine mismatch.
    let swapped = pin(
        "define i64 @sum({i64, i64} %p) {\n  %x = extractvalue {i64, i64} %p, 0\n  %y = extractvalue {i64, i64} %p, 1\n  %r = add i64 %x, %y\n  ret i64 %r\n}\n\
         define i64 @fwd({i64, i64} %p) {\n  %a = extractvalue {i64, i64} %p, 1\n  %b = extractvalue {i64, i64} %p, 0\n  %q0 = insertvalue {i64, i64} undef, i64 %a, 0\n  %q = insertvalue {i64, i64} %q0, i64 %b, 1\n  %r = call i64 @sum({i64, i64} %q)\n  ret i64 %r\n}",
    );
    assert_eq!(
        hir_outcome_with_structs(src, &pair_env(), &swapped),
        Outcome::Different
    );
}

#[test]
fn hir_leaf_compare_rejects_aggregate_misuse() {
    // A whole aggregate cannot stand in a scalar position: not as a
    // scalar-typed tail, an operand, or a call argument.
    // A real module, not the empty string — an empty module could not
    // tell a wrong HIR-side admission apart from a missing comparison
    // target. (The misuse bodies themselves do not codegen.)
    let real_ir = emitted_atom(&pair_source("p: Pair", "i64", "p.x"));
    for body in ["p", "p + 1", "g(p)"] {
        let src = pair_source("p: Pair", "i64", body);
        assert!(
            tv::validate_hir_pure_i64_leaf(
                &hir_atom(&src),
                &pair_env(),
                &BTreeMap::new(),
                &real_ir,
                false,
                Options::default()
            )
            .is_err(),
            "{body} stays outside the scalar subset"
        );
    }
    // A struct the map does not admit (e.g. non-i64 leaves) fails
    // closed at the signature gate.
    let unknown = "struct Triple { a: i64, b: i64, c: i64 }\natom f(t: Triple) -> i64 requires: true; ensures: true; body: { t.a };";
    assert!(
        tv::validate_hir_pure_i64_leaf(
            &hir_atom(unknown),
            &pair_env(),
            &call_sigs(unknown),
            &emitted_atom(unknown),
            false,
            Options::default()
        )
        .is_err(),
        "unmapped struct params stay outside the subset"
    );
    // Map entries that are not non-empty all-i64 records are dropped
    // from the admission, so a signature naming them fails closed the
    // same way.
    let wide = "struct Wide { a: i128 }
atom f(w: Wide) -> i64 requires: true; ensures: true; body: { w.a };";
    let wide_env = BTreeMap::from([(
        "Wide".to_string(),
        vec![("a".to_string(), "i128".to_string())],
    )]);
    assert!(
        tv::validate_hir_pure_i64_leaf(
            &hir_atom(wide),
            &wide_env,
            &call_sigs(wide),
            &emitted_atom(wide),
            false,
            Options::default()
        )
        .is_err(),
        "non-i64 leaf records are not admitted"
    );
    let empty = "struct Unit {}
atom f() -> Unit requires: true; ensures: true; body: { Unit {} };";
    let empty_env = BTreeMap::from([("Unit".to_string(), Vec::new())]);
    assert!(
        tv::validate_hir_pure_i64_leaf(
            &hir_atom(empty),
            &empty_env,
            &call_sigs(empty),
            &emitted_atom(empty),
            false,
            Options::default()
        )
        .is_err(),
        "empty records are not admitted"
    );
}

fn validate_transfer(ir: &str) -> tv::ModuleValidationReport {
    tv::validate_native_module(
        TRANSFER_SOURCE,
        ir,
        Options::default(),
        RefinedAdmission::CompositeV26,
    )
    .unwrap()
}

#[test]
fn decomposed_queries_accept_multi_edge_composite_transfer() {
    let report = validate_transfer(TRANSFER_IR);
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    assert!(report
        .functions
        .iter()
        .any(|function| function.function_name == "transfer"
            && function.outcome == Outcome::EquivalentWithinProfile));
}

#[test]
fn decomposed_queries_still_reject_a_divergent_transfer_edge() {
    let divergent =
        TRANSFER_IR.replace("%add_tmp33 = add i64 %2, 25", "%add_tmp33 = add i64 %2, 26");
    assert_ne!(divergent, TRANSFER_IR);
    assert_eq!(validate_transfer(&divergent).outcome, Outcome::Different);
}
