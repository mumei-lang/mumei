from __future__ import annotations

from pathlib import Path

import pytest
import yaml

_WORKFLOWS_DIR = Path(__file__).parent.parent / ".github" / "workflows"


def _workflow_files() -> list[Path]:
    if not _WORKFLOWS_DIR.exists():
        return []
    return sorted({*_WORKFLOWS_DIR.glob("*.yml"), *_WORKFLOWS_DIR.glob("*.yaml")})


@pytest.mark.parametrize(
    "workflow_file",
    _workflow_files(),
    ids=lambda p: p.name,
)
def test_workflow_jobs_have_timeout_minutes(workflow_file: Path) -> None:
    """Every job in .github/workflows must declare timeout-minutes."""
    data = yaml.safe_load(workflow_file.read_text(encoding="utf-8")) or {}
    jobs = data.get("jobs", {})
    assert jobs, f"{workflow_file.name} has no jobs"
    for job_name, job_config in jobs.items():
        assert "timeout-minutes" in job_config, (
            f"job {job_name!r} in {workflow_file.name} is missing timeout-minutes"
        )
