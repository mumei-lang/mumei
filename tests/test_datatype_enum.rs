use std::process::Command;

fn mumei_verify(file: &str) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_mumei");
    Command::new(bin)
        .arg("verify")
        .arg(file)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify {file}: {err}"))
}

#[test]
fn datatype_enum_match_verifies() {
    let output = mumei_verify("tests/test_datatype_enum.mm");
    assert!(
        output.status.success(),
        "finite-ADT enum verification should succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("Verification passed") && !combined.contains('❌'),
        "every atom should verify on the datatype path:\n{combined}"
    );
}

#[test]
fn enum_variant_resolution_is_deterministic_via_declared_type() {
    // IntList's `Cons = 0` collides with the prelude `List`'s `Cons = 1`;
    // `l: IntList` must resolve IntList's tags on every run.
    for _ in 0..3 {
        let output = mumei_verify("tests/test_enum_variant_resolution.mm");
        let combined = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.status.success() && combined.contains("Verification passed"),
            "declared-type resolution should be deterministic:\n{combined}"
        );
    }
}

#[test]
fn ambiguous_variant_match_fails_closed() {
    let output = mumei_verify("tests/test_enum_variant_resolution_negative.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && combined.contains("Ambiguous enum variant"),
        "unresolvable variant collision must fail closed:\n{combined}"
    );
}

#[test]
fn datatype_enum_missing_arm_fails_with_ctor_name() {
    let output = mumei_verify("tests/test_datatype_enum_negative.mm");
    assert!(
        !output.status.success(),
        "non-exhaustive enum match should fail verification"
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("Match is not exhaustive") && combined.contains("Color::Blue (tag=2)"),
        "expected uncovered-constructor counterexample naming 'Blue', got:\n{combined}"
    );
}

#[test]
fn datatype_enum_missing_payload_arm_shows_fields_and_tag() {
    let output = mumei_verify("tests/test_datatype_enum_payload_negative.mm");
    assert!(
        !output.status.success(),
        "non-exhaustive enum match should fail verification"
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("Match is not exhaustive")
            && combined.contains("Shape::Circle(i64=?) (tag=1)"),
        "expected payload-variant counterexample `(fields) (tag=N)`, got:\n{combined}"
    );
}

#[test]
fn shared_variant_names_resolve_to_the_target_enum() {
    // Two enums share variant names; the uncovered arm must be reported on the
    // target's own enum (A::X), not the other owner of the same-named variants.
    let output = mumei_verify("tests/test_enum_shared_variant_negative.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && combined.contains("A::X (tag=0)") && !combined.contains("B::"),
        "counterexample must name the target's enum, got:\n{combined}"
    );
}

#[test]
fn qualified_variant_arms_resolve_the_qualifier_enum() {
    let output = mumei_verify("tests/test_enum_qualified_pattern.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success() && combined.contains("Verification passed"),
        "qualified `E::V` arms should verify on datatype, Int-tag, and let-bound targets:\n{combined}"
    );
}

#[test]
fn qualified_arm_mismatching_target_type_fails_closed() {
    let output = mumei_verify("tests/test_enum_qualified_pattern_negative.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && combined.contains("belongs to enum"),
        "a `Qual::V` arm on a differently-typed target must fail closed:\n{combined}"
    );
}

#[test]
fn enum_named_payload_type_drives_binding_and_refinement() {
    // `On(b: bool)` must bind `b` as Bool (an unnormalized "b: bool" fell
    // back to Int) and `On(p: Port)` must assume the alias's conjuncts
    // under the arm's hit condition.
    let output = mumei_verify("tests/test_enum_named_payload.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success() && combined.contains("Verification passed"),
        "named bool/refined payloads should verify:\n{combined}"
    );
}

#[test]
fn enum_ctor_payload_must_satisfy_declared_field_type() {
    let output = mumei_verify("tests/test_enum_ctor_refined_negative.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && combined.contains("not satisfied"),
        "a payload violating its declared refined field type must be rejected:\n{combined}"
    );
}

#[test]
fn enum_ctor_violation_inside_reachable_match_arm_is_rejected() {
    let output = mumei_verify("tests/test_enum_ctor_refined_arm_negative.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && combined.contains("not satisfied"),
        "a violating payload in a reachable match arm must not verify vacuously:\n{combined}"
    );
}

#[test]
fn enum_tag_reads_verify_on_datatype_and_int_tag_paths() {
    let output = mumei_verify("tests/test_enum_tag_read.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success() && combined.contains("Verification passed"),
        ".tag on a bound enum value must read the variant index (datatype) or the tag int (Int-tag):\n{combined}"
    );
}

#[test]
fn enum_tag_in_callee_contract_uses_callee_enum() {
    let output = mumei_verify("tests/test_enum_tag_callee_context.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && combined.contains("caller"),
        "a callee's `.tag` must resolve the callee's enum, not a same-named caller binding — `caller`'s `result == 0` must be rejected:\n{combined}"
    );
    assert!(
        combined.contains("'caller_struct': verified")
            || combined.contains("'caller_struct': skipped"),
        "a callee's struct param must shadow a caller's same-named enum binding — `caller_struct`'s `h.v.tag` must resolve `Holder.v: Ev` and verify:\n{combined}"
    );
}

#[test]
fn enum_nested_composite_payload_leaves_verify() {
    let output = mumei_verify("tests/test_enum_nested_payload.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success() && combined.contains("Verification passed"),
        "a payload whose declared type is itself a struct must flatten into per-leaf bindings (b.inner.x) on ctor, match binding and field-assign paths:\n{combined}"
    );
}

#[test]
fn enum_nested_payload_dag_stays_bounded() {
    // A 30-level payload where every struct has two identical children
    // would take 2^30 leaf-walk visits — and all leaves are empty
    // structs, so the leaf cap alone never trips. The visit budget must
    // fail the flattening closed instead of hanging verification.
    let output = mumei_verify("tests/test_enum_nested_dag.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("leaf-walk visit budget"),
        "the visit-budget rejection must be observable — otherwise the test cannot tell the bounded walk ran:\n{combined}"
    );
    assert!(
        combined.contains("'dag_probe': verified")
            || combined.contains("'dag_probe': skipped"),
        "a deep shared-subtree payload must fail the flattening closed (scalar fallback), not hang or error:\n{combined}"
    );
}

#[test]
fn enum_generic_instance_match_and_ctor_verify() {
    // `match e { Wrap::W(v) => … }` on `e: Wrap<i64>` (and `Wrap<bool>`)
    // resolves the generic qualifier to the instance the declared type
    // pins — tester, selectors and `.tag` all work on the Datatype path.
    let output = mumei_verify("tests/test_enum_generic.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success() && combined.contains("Verification passed"),
        "a generic enum's monomorphized instances must verify under match/`.tag` with declared-type disambiguation:\n{combined}"
    );
}

#[test]
fn enum_generic_sole_instance_ctor_verifies() {
    // `Wrap::W(3)` resolves through the sole registered `Wrap<i64>`
    // instance — no declared binding type needed.
    let output = mumei_verify("tests/test_enum_generic_ctor.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success() && combined.contains("Verification passed"),
        "a generic-qualifier ctor with one registered instance must construct it:\n{combined}"
    );
}

#[test]
fn enum_generic_ambiguous_ctor_is_rejected() {
    // `Wrap::W(3)` with both `Wrap<i64>` and `Wrap<bool>` registered and
    // no declared type to disambiguate must fail closed.
    let output = mumei_verify("tests/test_enum_generic_ambiguous_negative.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && combined.contains("is ambiguous"),
        "an ambiguous generic-enum ctor must be rejected, not resolved at HashMap order:\n{combined}"
    );
}

#[test]
fn enum_ctor_payload_satisfying_field_type_verifies() {
    let output = mumei_verify("tests/test_enum_ctor_refined.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success() && combined.contains("Verification passed"),
        "payloads satisfying the declared field type — including inside match arms — must verify:\n{combined}"
    );
}

#[test]
fn match_target_field_const_prefers_caller_binding() {
    let output = mumei_verify("tests/test_enum_match_arg_field_shadowed.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success() && combined.contains("Verification passed"),
        "`s_v` projects the caller's `s: HolderA` — the callee's same-named `s: HolderB` param must not shadow it:\n{combined}"
    );
}

#[test]
fn enum_nested_payload_alias_empty_struct_does_not_panic() {
    let output = mumei_verify("tests/test_enum_nested_alias_empty.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success() && combined.contains("Verification passed"),
        "an alias to an empty struct flattens to zero leaves — the bound handle must not index an empty slice:\n{combined}"
    );
}

#[test]
fn enum_generic_binding_tag_reads_instance_domain() {
    let output = mumei_verify("tests/test_enum_generic_ctor.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success() && combined.contains("Verification passed"),
        "`let e = Wrap::W(3)` must bind `e` to the sole instance `Wrap<i64>` so `e.tag == 0` verifies:\n{combined}"
    );
}

#[test]
fn enum_generic_ctor_with_no_instance_does_not_panic() {
    let output = mumei_verify("tests/test_enum_generic_no_instance.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success()
            && !combined.contains("panicked")
            && !combined.contains("internal error"),
        "a generic ctor with zero registered instances must fail closed — the match rejects, never panics and never verifies:\n{combined}"
    );
}
