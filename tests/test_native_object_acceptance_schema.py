"""Representation gates for the native object-route acceptance descriptor schema."""
from __future__ import annotations

import json
from copy import deepcopy
from pathlib import Path

import jsonschema
import pytest

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = json.loads(
    (ROOT / "schema/native-object-acceptance.schema.json").read_text(encoding="utf-8")
)
FIXTURE = json.loads(
    (
        ROOT / "tests/fixtures/verification-evidence/native_object_acceptance_shape.json"
    ).read_text(encoding="utf-8")
)
VALIDATOR = jsonschema.Draft202012Validator(SCHEMA)


def descriptor() -> dict:
    return deepcopy(FIXTURE)


def test_schema_is_valid_and_fixture_conforms() -> None:
    VALIDATOR.check_schema(SCHEMA)
    VALIDATOR.validate(descriptor())


def test_without_object_link_scope_conforms() -> None:
    doc = descriptor()
    doc["scope"] = "native_bundle_object_translation_correspondence_without_object_link"
    doc["checks_performed"].remove("object_link_correspondence")
    doc["checks_not_performed"].append("object_link_correspondence")
    doc["source_machine_tv"]["object_link_correspondence"] = (
        "unchecked_not_translation_validated"
    )
    doc["source_machine_tv"]["unchecked_domains"].append("object_link_correspondence")
    doc["object_translation"]["transformations"][0]["result"] = (
        "unchecked_not_translation_validated"
    )
    doc["object_translation"]["transformations"][1]["result"] = (
        "unchecked_not_translation_validated"
    )
    VALIDATOR.validate(doc)


@pytest.mark.parametrize("field", SCHEMA["required"])
def test_required_fields_cannot_disappear(field: str) -> None:
    doc = descriptor()
    del doc[field]
    assert not VALIDATOR.is_valid(doc)


def test_unknown_top_level_field_is_rejected() -> None:
    doc = descriptor()
    doc["extra"] = True
    assert not VALIDATOR.is_valid(doc)


@pytest.mark.parametrize(
    ("mutator",),
    [
        (lambda d: d.__setitem__("schema_version", "mumei.native-evidence-acceptance/v2"),),
        (lambda d: d.__setitem__("acceptance", "not_evaluated"),),
        (lambda d: d.__setitem__("accepted", False),),
        (lambda d: d.__setitem__("scope", "native_bundle_object_machine_code_translation_correspondence"),),
        (lambda d: d.__setitem__("build_id", "not-a-sha256"),),
        (lambda d: d.__setitem__("evidence_fingerprint", "A" * 64),),
        (lambda d: d.__setitem__("functions", {}),),
        (lambda d: d["functions"].__setitem__("bad", {"outcome": "different"}),),
        (lambda d: d["functions"].__setitem__("bad", {"outcome": "unknown"}),),
        (lambda d: d["functions"].__setitem__("bad", {"outcome": "outside_profile", "reason": "cyclic_control_flow"}),),
        (lambda d: d["object_translation"].__setitem__("status", "metadata_only"),),
        (lambda d: d["object_translation"]["correspondence"].__setitem__("status", "unchecked"),),
        (lambda d: d["object_translation"]["correspondence"]["checks"][0].__setitem__("result", "unchecked"),),
        (lambda d: d["object_translation"]["correspondence"]["checks"][0].__setitem__("result", "inconsistent"),),
        (lambda d: d["source_machine_tv"].__setitem__("status", "unchecked_not_translation_validated"),),
        (lambda d: d["source_machine_tv"].__setitem__("object_link_correspondence", "unchecked_not_translation_validated"),),
        (lambda d: d["source_machine_tv"]["non_equivalent_functions"].append({"name": "f", "outcome": "different", "reason": "not_a_source_atom"}),),
        (lambda d: d["object_translation"]["correspondence"]["checks"][0].__setitem__("item", "unrelated_check"),),
    ],
)
def test_invalid_representations_are_rejected(mutator) -> None:
    doc = descriptor()
    mutator(doc)
    assert not VALIDATOR.is_valid(doc)
