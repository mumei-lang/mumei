from __future__ import annotations

from pathlib import Path

import yaml

_WORKFLOWS_DIR = Path(__file__).parent.parent / ".github" / "workflows"


def test_jekyll_copies_installer_before_uploading_pages_artifact() -> None:
    workflow_file = _WORKFLOWS_DIR / "jekyll-gh-pages.yml"
    workflow = yaml.safe_load(workflow_file.read_text(encoding="utf-8"))
    steps = workflow["jobs"]["build"]["steps"]

    build_index = next(
        index for index, step in enumerate(steps) if step.get("name") == "Build with Jekyll"
    )
    copy_index = next(
        index
        for index, step in enumerate(steps)
        if step.get("run") == "sudo cp -p scripts/install.sh _site/install.sh"
    )
    upload_index = next(
        index
        for index, step in enumerate(steps)
        if step.get("uses", "").startswith("actions/upload-pages-artifact@")
    )

    assert build_index < copy_index < upload_index


def test_only_jekyll_workflow_deploys_pages() -> None:
    workflow_files = sorted(
        {*_WORKFLOWS_DIR.glob("*.yml"), *_WORKFLOWS_DIR.glob("*.yaml")}
    )

    for workflow_file in workflow_files:
        if workflow_file.name == "jekyll-gh-pages.yml":
            continue

        workflow = yaml.safe_load(workflow_file.read_text(encoding="utf-8")) or {}
        for job_name, job in workflow.get("jobs", {}).items():
            for step in job.get("steps", []):
                assert not step.get("uses", "").startswith("actions/deploy-pages@"), (
                    f"{workflow_file.name} job {job_name!r} deploys Pages"
                )
