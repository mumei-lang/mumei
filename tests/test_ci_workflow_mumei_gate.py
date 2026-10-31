from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest
import yaml

_WORKFLOW_FILE = (
    Path(__file__).parent.parent / ".github" / "workflows" / "mumei-gate.yml"
)


def _workflow() -> dict:
    return yaml.safe_load(_WORKFLOW_FILE.read_text(encoding="utf-8"))


def _gate_step() -> dict:
    workflow = _workflow()
    return next(
        step
        for step in workflow["jobs"]["gate"]["steps"]
        if step.get("name") == "Run Claim Book and Charter gates"
    )


def _run_gate(
    tmp_path: Path,
    *,
    sources: str = "a.mm",
    certificates: str = "",
    allow_lean_verified: bool = False,
    require_grade: str = "",
    require_discriminating: bool = False,
    require_complete: bool = False,
    observations: str = "",
    charter: bool = False,
    fail_claims_source: str = "",
) -> tuple[subprocess.CompletedProcess[str], list[list[str]], str, Path, Path]:
    project = tmp_path / "project"
    project.mkdir()
    for source in sources.split():
        source_path = project / source
        source_path.parent.mkdir(parents=True, exist_ok=True)
        source_path.write_text("dummy source\n", encoding="utf-8")

    for certificate in ("certs/first.json", "certs/second.json"):
        certificate_path = project / certificate
        certificate_path.parent.mkdir(parents=True, exist_ok=True)
        certificate_path.write_text(f"{certificate}\n", encoding="utf-8")

    runner_temp = tmp_path / "runner-temp"
    runner_temp.mkdir()
    summary_path = tmp_path / "summary.md"
    summary_path.write_text("", encoding="utf-8")
    log_path = tmp_path / "mumei-argv.jsonl"
    log_path.write_text("", encoding="utf-8")
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    mumei_shim = bin_dir / "mumei"
    mumei_shim.write_text(
        f"#!{sys.executable}\n"
        "import json\n"
        "import os\n"
        "import sys\n"
        "from pathlib import Path\n"
        "argv = sys.argv[1:]\n"
        "with Path(os.environ['MUMEI_LOG']).open('a', encoding='utf-8') as log:\n"
        "    log.write(json.dumps(argv) + '\\n')\n"
        "if argv[0] == 'verify':\n"
        "    output = Path(argv[argv.index('--output') + 1])\n"
        "    output.parent.mkdir(parents=True, exist_ok=True)\n"
        "    output.write_text('fresh certificate\\n', encoding='utf-8')\n"
        "if (argv[0] == 'claims' and argv[1] == os.environ.get('FAIL_CLAIMS_SOURCE')\n"
        "        and os.environ.get('FAIL_CLAIMS_SOURCE')):\n"
        "    sys.exit(1)\n",
        encoding="utf-8",
    )
    mumei_shim.chmod(0o755)

    env = os.environ.copy()
    env.update(
        {
            "PATH": f"{bin_dir}{os.pathsep}{env.get('PATH', '')}",
            "RUNNER_TEMP": str(runner_temp),
            "GITHUB_STEP_SUMMARY": str(summary_path),
            "WORKING_DIRECTORY": str(project),
            "SOURCES": sources,
            "REQUIRE_GRADE": require_grade,
            "REQUIRE_DISCRIMINATING": str(require_discriminating).lower(),
            "REQUIRE_COMPLETE": str(require_complete).lower(),
            "OBSERVATIONS": observations,
            "CHARTER": str(charter).lower(),
            "ALLOW_LEAN_VERIFIED": str(allow_lean_verified).lower(),
            "CERTIFICATES": certificates,
            "MUMEI_LOG": str(log_path),
            "FAIL_CLAIMS_SOURCE": fail_claims_source,
        }
    )
    result = subprocess.run(
        ["bash", "-e", "-c", _gate_step()["run"]],
        cwd=project,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    calls = [
        json.loads(line)
        for line in log_path.read_text(encoding="utf-8").splitlines()
    ]
    return (
        result,
        calls,
        summary_path.read_text(encoding="utf-8"),
        runner_temp,
        project,
    )


def _out_dir(runner_temp: Path, source: str) -> Path:
    slug = source.removesuffix(".mm").replace("/", "__")
    return runner_temp / "mumei-gate" / slug


def _command(calls: list[list[str]], command: str, source: str) -> list[str]:
    return next(call for call in calls if call[0] == command and source in call)


def test_default_new_inputs_keep_fresh_certificate_commands(tmp_path: Path) -> None:
    result, calls, summary, runner_temp, _ = _run_gate(
        tmp_path,
        require_grade="G4",
        require_discriminating=True,
        observations="build/observations.json",
        charter=True,
    )

    out = _out_dir(runner_temp, "a.mm")
    fresh_cert = str(out / "a.proof.json")
    assert result.returncode == 0, result.stderr
    assert calls == [
        ["verify", "a.mm", "--proof-cert", "--output", fresh_cert],
        [
            "claims",
            "a.mm",
            "--cert",
            fresh_cert,
            "--format",
            "both",
            "--out-dir",
            str(out),
            "--require-grade",
            "G4",
            "--require-discriminating",
            "--observations",
            "build/observations.json",
        ],
        [
            "charter",
            "check",
            "a.mm",
            "--cert",
            fresh_cert,
            "--require-grade",
            "G4",
            "--json",
        ],
    ]
    assert f"- Claim Book certificate: `fresh`" in summary


@pytest.mark.parametrize("charter", [False, True])
def test_allow_lean_verified_is_added_to_enabled_commands(
    tmp_path: Path, charter: bool
) -> None:
    result, calls, _, _, _ = _run_gate(
        tmp_path,
        allow_lean_verified=True,
        require_grade="G5",
        charter=charter,
    )

    assert result.returncode == 0, result.stderr
    commands = {call[0]: call for call in calls}
    assert "--allow-lean-verified" in commands["verify"]
    assert "--allow-lean-verified" in commands["claims"]
    if charter:
        assert "--allow-lean-verified" in commands["charter"]
    else:
        assert "charter" not in commands


@pytest.mark.parametrize("enabled", [False, True])
@pytest.mark.parametrize("other_gates", [False, True])
def test_dependency_coverage_gate_is_opt_in_and_only_changes_claims(
    tmp_path: Path, enabled: bool, other_gates: bool
) -> None:
    result, calls, _, _, _ = _run_gate(
        tmp_path,
        sources="a.mm sub/b.mm",
        require_complete=enabled,
        require_grade="G4" if other_gates else "",
        require_discriminating=other_gates,
        charter=True,
    )

    assert result.returncode == 0, result.stderr
    for call in calls:
        assert ("--require-complete" in call) == (enabled and call[0] == "claims")
        if call[0] == "claims":
            assert ("--require-grade" in call) == other_gates
            assert ("--require-discriminating" in call) == other_gates


def test_supplied_certificate_is_copied_and_used_without_lean_opt_in(
    tmp_path: Path,
) -> None:
    result, calls, summary, runner_temp, project = _run_gate(
        tmp_path,
        certificates="certs/first.json",
        charter=True,
    )

    out = _out_dir(runner_temp, "a.mm")
    fresh_cert = str(out / "a.proof.json")
    supplied_cert = out / "a.supplied.proof.json"
    assert result.returncode == 0, result.stderr
    assert _command(calls, "verify", "a.mm") == [
        "verify",
        "a.mm",
        "--proof-cert",
        "--output",
        fresh_cert,
    ]
    assert _command(calls, "claims", "a.mm") == [
        "claims",
        "a.mm",
        "--cert",
        str(supplied_cert),
        "--format",
        "both",
        "--out-dir",
        str(out),
    ]
    assert _command(calls, "charter", "a.mm") == [
        "charter",
        "check",
        "a.mm",
        "--cert",
        str(supplied_cert),
        "--json",
    ]
    assert "--allow-lean-verified" not in " ".join(
        " ".join(call) for call in calls
    )
    assert supplied_cert.read_text(encoding="utf-8") == (
        project / "certs/first.json"
    ).read_text(encoding="utf-8")
    assert Path(fresh_cert).is_file()
    assert "- Claim Book certificate: `supplied: certs/first.json`" in summary


def test_two_supplied_certificates_pair_with_sources_in_order(tmp_path: Path) -> None:
    result, calls, summary, runner_temp, _ = _run_gate(
        tmp_path,
        sources="a.mm sub/b.mm",
        certificates="certs/first.json certs/second.json",
        allow_lean_verified=True,
        require_grade="G5",
        charter=True,
    )

    assert result.returncode == 0, result.stderr
    for source, input_certificate, slug in (
        ("a.mm", "certs/first.json", "a"),
        ("sub/b.mm", "certs/second.json", "sub__b"),
    ):
        out = runner_temp / "mumei-gate" / slug
        fresh_cert = str(out / f"{slug}.proof.json")
        supplied_cert = str(out / f"{slug}.supplied.proof.json")
        assert _command(calls, "verify", source) == [
            "verify",
            source,
            "--proof-cert",
            "--output",
            fresh_cert,
            "--allow-lean-verified",
        ]
        assert _command(calls, "claims", source) == [
            "claims",
            source,
            "--cert",
            supplied_cert,
            "--format",
            "both",
            "--out-dir",
            str(out),
            "--allow-lean-verified",
            "--require-grade",
            "G5",
        ]
        assert _command(calls, "charter", source) == [
            "charter",
            "check",
            source,
            "--cert",
            supplied_cert,
            "--allow-lean-verified",
            "--require-grade",
            "G5",
            "--json",
        ]
        assert Path(supplied_cert).is_file()
        assert f"- Claim Book certificate: `supplied: {input_certificate}`" in summary


@pytest.mark.parametrize(
    "certificates",
    [
        "certs/first.json",
        "certs/first.json certs/second.json certs/third.json",
    ],
)
def test_certificate_count_mismatch_fails_before_mumei(
    tmp_path: Path, certificates: str
) -> None:
    result, calls, _, _, _ = _run_gate(
        tmp_path, sources="a.mm sub/b.mm", certificates=certificates
    )

    assert result.returncode == 1
    assert "Certificate count" in result.stderr
    assert calls == []


def test_missing_certificate_fails_before_mumei(tmp_path: Path) -> None:
    result, calls, _, _, _ = _run_gate(
        tmp_path, certificates="certs/missing.json"
    )

    assert result.returncode == 1
    assert "does not exist or is not a file" in result.stderr
    assert calls == []


def test_claims_failure_does_not_stop_processing_later_sources(
    tmp_path: Path,
) -> None:
    result, calls, _, _, _ = _run_gate(
        tmp_path,
        sources="a.mm sub/b.mm",
        charter=True,
        require_complete=True,
        fail_claims_source="a.mm",
    )

    assert result.returncode == 1
    assert [call[0] for call in calls] == [
        "verify",
        "claims",
        "charter",
        "verify",
        "claims",
        "charter",
    ]
    assert calls[-2][1] == "sub/b.mm"
    assert "--require-complete" in calls[-2]


def test_workflow_declares_and_wires_new_inputs() -> None:
    workflow = _workflow()
    on = workflow.get("on", workflow.get(True, {}))
    inputs = on["workflow_call"]["inputs"]
    steps = workflow["jobs"]["gate"]["steps"]
    gate_step = _gate_step()

    assert inputs["allow-lean-verified"]["type"] == "boolean"
    assert inputs["allow-lean-verified"]["default"] is False
    assert (
        inputs["allow-lean-verified"]["description"]
        == "Accept current Lean evidence for imports and Claim Book grades; required for G5."
    )
    assert inputs["certificates"]["type"] == "string"
    assert inputs["certificates"]["default"] == ""
    assert (
        inputs["certificates"]["description"]
        == "Optional whitespace-separated proof certificates, one per source in the same order, relative to working-directory; claims and charter read these instead of the fresh certificate."
    )
    assert gate_step["env"]["ALLOW_LEAN_VERIFIED"] == (
        "${{ inputs.allow-lean-verified }}"
    )
    assert gate_step["env"]["CERTIFICATES"] == "${{ inputs.certificates }}"
    assert inputs["require-complete"]["type"] == "boolean"
    assert inputs["require-complete"]["default"] is False
    assert gate_step["env"]["REQUIRE_COMPLETE"] == "${{ inputs.require-complete }}"
    upload = next(step for step in steps if "upload-artifact" in step.get("uses", ""))
    assert upload["if"] == "always()"

    preflight_index = next(
        index
        for index, step in enumerate(steps)
        if step.get("name") == "Validate gate sources and certificates"
    )
    install_index = next(
        index
        for index, step in enumerate(steps)
        if step.get("name") == "Install Mumei"
    )
    preflight = steps[preflight_index]
    assert preflight_index < install_index
    assert "Certificate count" in preflight["run"]
    assert '[[ ! -f "$certificate_path" ]]' in preflight["run"]
