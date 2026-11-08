"""Representation gates only: these synthetic records establish no proof truth."""
from __future__ import annotations

import itertools
import json
from copy import deepcopy
from pathlib import Path

import jsonschema
import pytest
import yaml

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = json.loads(
    (ROOT / "schema/verification-evidence.schema.json").read_text(encoding="utf-8")
)
CASES = json.loads(
    (ROOT / "tests/fixtures/verification-evidence/synthetic_cases.json").read_text(
        encoding="utf-8"
    )
)["cases"]
VALIDATOR = jsonschema.Draft202012Validator(SCHEMA)


def evidence(case: int = 0) -> dict:
    return deepcopy(CASES[case]["evidence"])


def change(record: dict, path: str, value: object) -> dict:
    target = record
    parts = path.split(".")
    for part in parts[:-1]:
        target = target[part]
    target[parts[-1]] = value
    return record


def test_schema_is_valid_and_fixtures_are_present() -> None:
    VALIDATOR.check_schema(SCHEMA)
    assert len(CASES) == 8
    assert len({case["name"] for case in CASES}) == len(CASES)


def test_architecture_contract_is_in_the_vocabulary_gate() -> None:
    from scripts.check_contract_vocabulary import SYNC_DOCS

    assert ROOT / "docs/VERIFICATION_NATIVE.md" in SYNC_DOCS


@pytest.mark.parametrize("case", CASES, ids=lambda case: case["name"])
def test_synthetic_records_have_valid_representation(case: dict) -> None:
    VALIDATOR.validate(case["evidence"])


def test_normalized_native_cli_shape_conforms_without_proof_acceptance() -> None:
    # Rust CLI regressions match this normalized shape; hashes/versions are placeholders.
    report = json.loads(
        (ROOT / "tests/fixtures/verification-evidence/native_consumer_shape.json").read_text(
            encoding="utf-8"
        )
    )
    assert report["schema_version"] == "mumei.native-consumer-evidence/v1"
    assert report["acceptance"] == "not_evaluated"
    assert len(report["evidence"]) == 3
    for record in report["evidence"]:
        VALIDATOR.validate(record)
    assert [record["method"] for record in report["evidence"]] == [
        "identity_check", "deductive_proof", "deductive_proof"
    ]
    assert report["evidence"][0]["correspondence"]["status"] == "unverified"
    assert report["evidence"][1]["correspondence"]["status"] == "assumed"
    assert report["evidence"][2]["correspondence"]["status"] == "validated"


@pytest.mark.parametrize("field", SCHEMA["required"])
def test_required_information_cannot_disappear(field: str) -> None:
    record = evidence()
    del record[field]
    assert not VALIDATOR.is_valid(record)


@pytest.mark.parametrize(
    ("path", "value"),
    [
        ("schema_version", "mumei.verification-evidence/v2"),
        ("method", "solver_success"),
        ("result", "verified"),
        ("execution", "ok"),
        ("subject.kind", "anything"),
        ("subject.sha256", "not-a-hash"),
        ("subject.sha256", "a" * 63),
        ("subject.sha256", "a" * 64 + "\n"),
        ("subject.sha256", "\n" + "a" * 64),
        ("subject.sha256", "a" * 64 + " "),
        ("specification.sha256", "A" * 64),
        ("provenance.configuration_fingerprint", ""),
        ("provenance.configuration_fingerprint", "a" * 64 + "\n"),
        ("provenance.bridge_lemma_hash", "stale"),
        ("semantics.integer_model", "machine-int"),
        ("semantics.profile", ""),
        ("semantics.target_triple", ""),
        ("scope.kind", "all-programs"),
        ("scope.bounds", [""]),
        ("scope.bounds", ["same", "same"]),
        ("scope.bounds", ["unexpected unbounded-scope bound"]),
        ("correspondence.relation", "equivalent-enough"),
        ("correspondence.status", "probably"),
        ("assumptions", [""]),
        ("exclusions", [1]),
        ("artifact_paths", [""]),
        ("artifact_paths", ["same", "same"]),
        ("provenance.adapter.protocol_version", ""),
        ("provenance.tool.version", None),
        ("provenance.translator_version", ""),
    ],
)
def test_invalid_fields_are_rejected(path: str, value: object) -> None:
    assert not VALIDATOR.is_valid(change(evidence(), path, value))


@pytest.mark.parametrize("execution", ["timeout", "resource_limit", "tool_error", "not_run"])
@pytest.mark.parametrize(
    "result", ["established", "counterexample", "observed_no_failure", "finding", "inconsistent"]
)
def test_incomplete_execution_cannot_claim_a_completed_result(
    execution: str, result: str
) -> None:
    record = evidence()
    record.update(execution=execution, result=result)
    assert not VALIDATOR.is_valid(record)


@pytest.mark.parametrize("execution", ["timeout", "resource_limit", "tool_error", "not_run"])
@pytest.mark.parametrize("result", ["unknown", "unsupported", "not_checked"])
def test_incomplete_execution_can_preserve_an_inconclusive_result(
    execution: str, result: str
) -> None:
    record = evidence()
    record.update(execution=execution, result=result)
    VALIDATOR.validate(record)


@pytest.mark.parametrize("case", [2, 3, 5])
def test_method_cannot_silently_change_scope(case: int) -> None:
    record = evidence(case)
    record["scope"] = {"kind": "unbounded", "bounds": []}
    assert not VALIDATOR.is_valid(record)


@pytest.mark.parametrize("case", [2, 3])
def test_finite_or_bounded_checks_need_explicit_limits(case: int) -> None:
    record = evidence(case)
    record["scope"]["bounds"] = []
    assert not VALIDATOR.is_valid(record)


@pytest.mark.parametrize("case", [2, 6])
def test_finite_tests_and_heuristics_cannot_claim_established(case: int) -> None:
    record = evidence(case)
    record["result"] = "established"
    assert not VALIDATOR.is_valid(record)


@pytest.mark.parametrize("case", [5, 6])
def test_identity_and_heuristics_cannot_claim_counterexamples(case: int) -> None:
    record = evidence(case)
    record["result"] = "counterexample"
    assert not VALIDATOR.is_valid(record)


def test_abstract_interpretation_warning_keeps_its_method_without_claiming_a_witness() -> None:
    record = evidence()
    record.update(method="abstract_interpretation", result="finding")
    VALIDATOR.validate(record)
    record["result"] = "counterexample"
    assert not VALIDATOR.is_valid(record)


def test_unchecked_port_proof_stays_about_the_port() -> None:
    record = evidence(1)
    VALIDATOR.validate(record)
    assert record["subject"]["kind"] == "model"
    assert record["correspondence"]["status"] == "unverified"
    record["correspondence"]["status"] = "validated"
    assert not VALIDATOR.is_valid(record)


@pytest.mark.parametrize("status", ["validated", "assumed", "unverified"])
def test_unsupported_relation_cannot_be_treated_as_supported(status: str) -> None:
    record = evidence(7)
    record["correspondence"]["status"] = status
    assert not VALIDATOR.is_valid(record)


@pytest.mark.parametrize("result", ["established", "counterexample", "observed_no_failure"])
def test_partial_translation_cannot_claim_a_conclusive_result(result: str) -> None:
    record = evidence(7)
    record["result"] = result
    assert not VALIDATOR.is_valid(record)


def test_unsupported_status_requires_unsupported_relation() -> None:
    record = evidence()
    record["result"] = "unknown"
    record["correspondence"]["status"] = "unsupported"
    assert not VALIDATOR.is_valid(record)


def test_schema_has_no_grade_or_existing_certificate_promotion_field() -> None:
    for field in ["grade", "verified", "all_verified", "lean_verified", "verification_status"]:
        record = evidence()
        record[field] = True
        assert not VALIDATOR.is_valid(record)


@pytest.mark.parametrize(
    ("path", "value"),
    [
        ("environment", {"FAKE_API_KEY": "FAKE_SECRET_SENTINEL"}),
        ("provenance.raw_configuration", {"token": "FAKE_SECRET_SENTINEL"}),
        ("provenance.tool.command", ["tool", "--password", "FAKE_SECRET_SENTINEL"]),
        ("provenance.adapter.endpoint", "https://fake.invalid/?key=FAKE_SECRET_SENTINEL"),
    ],
)
def test_record_has_no_raw_configuration_or_environment_channel(
    path: str, value: object
) -> None:
    assert not VALIDATOR.is_valid(change(evidence(), path, value))


@pytest.mark.parametrize(
    "parent",
    ["subject", "specification", "semantics", "scope", "correspondence", "provenance",
     "provenance.adapter", "provenance.tool"],
)
def test_unknown_nested_fields_are_not_silently_ignored(parent: str) -> None:
    assert not VALIDATOR.is_valid(change(evidence(), f"{parent}.unexpected", True))


def test_all_method_result_pairs_obey_the_representation_contract() -> None:
    allowed = {
        "deductive_proof": {"established", "counterexample"},
        "abstract_interpretation": {"established", "finding"},
        "bounded_model_checking": {"established", "counterexample"},
        "finite_execution": {"observed_no_failure", "counterexample"},
        "static_heuristic": {"finding"},
        "identity_check": {"established"},
    }
    neutral = {"unknown", "unsupported", "not_checked", "inconsistent"}
    assert set(SCHEMA["properties"]["method"]["enum"]) == set(allowed)
    scopes = {
        "bounded_model_checking": ("bounded", ["harness bound"]),
        "finite_execution": ("finite", ["sample inputs"]),
        "identity_check": ("identity", []),
    }
    for method, result in itertools.product(allowed, SCHEMA["properties"]["result"]["enum"]):
        record = evidence()
        record.update(method=method, result=result)
        kind, bounds = scopes.get(method, ("unbounded", []))
        record["scope"] = {"kind": kind, "bounds": bounds}
        assert VALIDATOR.is_valid(record) == (result in allowed[method] | neutral), (
            method, result
        )


def test_evidence_gate_runs_when_only_schema_or_fixtures_change() -> None:
    workflow = yaml.safe_load(
        (ROOT / ".github/workflows/contract-vocabulary.yml").read_text(encoding="utf-8")
    )
    triggers = workflow.get("on", workflow.get(True))
    required = {
        "schema/verification-evidence.schema.json",
        "tests/fixtures/verification-evidence/**",
        "tests/test_verification_evidence_schema.py",
        "schema/proof-cert.schema.json",
        "tests/fixtures/proof-cert/**",
        "tests/test_proof_cert_schema.py",
    }
    for event in ["push", "pull_request"]:
        assert required <= set(triggers[event]["paths"])
    steps = workflow["jobs"]["contract-vocabulary"]["steps"]
    runs = "\n".join(step.get("run", "") for step in steps)
    assert "jsonschema" in runs
    assert "tests/test_verification_evidence_schema.py" in runs
