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
    publisher_prefixes = (
        "actions/deploy-pages@",
        "actions/upload-pages-artifact@",
        "peaceiris/actions-gh-pages@",
    )
    workflow_files = sorted(
        {*_WORKFLOWS_DIR.glob("*.yml"), *_WORKFLOWS_DIR.glob("*.yaml")}
    )

    for workflow_file in workflow_files:
        if workflow_file.name == "jekyll-gh-pages.yml":
            continue

        workflow = yaml.safe_load(workflow_file.read_text(encoding="utf-8")) or {}
        for job_name, job in workflow.get("jobs", {}).items():
            for step in job.get("steps", []):
                assert not step.get("uses", "").startswith(publisher_prefixes), (
                    f"{workflow_file.name} job {job_name!r} deploys Pages"
                )


def test_release_waits_for_jekyll_deployment_and_published_installer() -> None:
    workflow_file = _WORKFLOWS_DIR / "release.yml"
    workflow = yaml.safe_load(workflow_file.read_text(encoding="utf-8"))
    steps = workflow["jobs"]["redeploy-pages"]["steps"]
    run_text = "\n".join(step.get("run", "") for step in steps)

    dispatch_index = run_text.index("gh workflow run jekyll-gh-pages.yml")
    watch_index = run_text.index("gh run watch")
    exit_status_index = run_text.index("--exit-status")
    assert dispatch_index < watch_index < exit_status_index

    dispatch_step_index = next(
        index
        for index, step in enumerate(steps)
        if step.get("name") == "Dispatch Jekyll deployment and wait for it"
    )
    installer_check_index = next(
        index
        for index, step in enumerate(steps)
        if "curl -fsSIL https://mumei-lang.github.io/mumei/install.sh"
        in step.get("run", "")
    )
    assert installer_check_index > dispatch_step_index
