"""Representation gate only: a valid envelope establishes no producer or key
authenticity, freshness, revocation status, claim truth or acceptance."""
from __future__ import annotations

import json
from copy import deepcopy
from pathlib import Path

import jsonschema
import pytest

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = json.loads(
    (ROOT / "schema/producer-signature.schema.json").read_text(encoding="utf-8")
)
VALIDATOR = jsonschema.Draft202012Validator(SCHEMA)

ENVELOPE = {
    "schema_version": "mumei.producer-signature/v1",
    "subject_digest": "a" * 64,
    "algorithm": "ed25519",
    "signature": "b" * 128,
    "producer_key_id": "producer-a",
}


def test_schema_is_valid_and_accepts_the_closed_envelope() -> None:
    VALIDATOR.check_schema(SCHEMA)
    assert VALIDATOR.is_valid(ENVELOPE)


@pytest.mark.parametrize(
    "field,value",
    [
        ("schema_version", "mumei.producer-signature/v0"),
        ("schema_version", "mumei.producer-signature/v2"),
        ("schema_version", 1),
        ("subject_digest", "a" * 63),
        ("subject_digest", "a" * 65),
        ("subject_digest", "A" * 64),  # uppercase
        ("subject_digest", "g" * 64),
        ("subject_digest", "a" * 64 + "\n"),  # `$` matches before a trailing newline
        ("subject_digest", 0),
        ("algorithm", "ed25519ph"),
        ("algorithm", "rsa"),
        ("algorithm", 1),
        ("signature", "b" * 127),
        ("signature", "b" * 129),
        ("signature", "B" * 128),  # uppercase
        ("signature", "z" * 128),
        ("signature", "b" * 128 + "\n"),  # `$` matches before a trailing newline
        ("signature", 0),
        ("producer_key_id", ""),
        ("producer_key_id", None),
        ("producer_key_id", 1),
        ("producer_key_id", ["producer-a"]),
    ],
)
def test_envelope_rejects_out_of_shape_fields(field: str, value: object) -> None:
    changed = deepcopy(ENVELOPE)
    changed[field] = value
    assert not VALIDATOR.is_valid(changed), (field, value)


def test_envelope_is_closed_and_every_field_is_required() -> None:
    for field in list(ENVELOPE):
        changed = deepcopy(ENVELOPE)
        del changed[field]
        assert not VALIDATOR.is_valid(changed), field
    changed = deepcopy(ENVELOPE)
    changed["extra"] = True
    assert not VALIDATOR.is_valid(changed)


def test_produced_at_is_the_only_optional_field() -> None:
    stamped = deepcopy(ENVELOPE)
    stamped["produced_at"] = "2026-11-18T12:00:00Z"
    assert VALIDATOR.is_valid(stamped)
    for bad in (0, None, ["2026-11-18T12:00:00Z"], ""):
        changed = deepcopy(ENVELOPE)
        changed["produced_at"] = bad
        assert not VALIDATOR.is_valid(changed), bad
