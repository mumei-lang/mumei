from __future__ import annotations

import importlib.util
import re
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
RELEASE_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "release.yml"
INSTALL_SCRIPT = REPO_ROOT / "scripts" / "install.sh"
GENERATE_FORMULA = REPO_ROOT / "scripts" / "generate_formula.py"


def load_generate_formula_module():
    spec = importlib.util.spec_from_file_location("generate_formula", GENERATE_FORMULA)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def release_matrix_names() -> set[str]:
    workflow = RELEASE_WORKFLOW.read_text()
    return set(re.findall(r"^\s+name: (mumei-[a-zA-Z0-9_-]+)$", workflow, re.MULTILINE))


def test_formula_strips_v_prefix_but_uses_tagged_release_urls() -> None:
    module = load_generate_formula_module()
    formula = module.render_formula(
        version="v0.6.3",
        sha_x86_mac="x86-mac-sha",
        sha_arm_mac="arm-mac-sha",
        sha_x86_linux="x86-linux-sha",
        sha_arm_linux="arm-linux-sha",
    )

    assert 'version "0.6.3"' in formula
    assert "/releases/download/0.6.3/" not in formula
    assert "/releases/download/vv0.6.3/" not in formula
    for asset in {
        "mumei-x86_64-apple-darwin.tar.gz",
        "mumei-aarch64-apple-darwin.tar.gz",
        "mumei-x86_64-unknown-linux-gnu.tar.gz",
        "mumei-aarch64-unknown-linux-gnu.tar.gz",
    }:
        assert f"https://github.com/mumei-lang/mumei/releases/download/v0.6.3/{asset}" in formula


def test_formula_accepts_unprefixed_versions_with_same_url_shape() -> None:
    module = load_generate_formula_module()
    formula = module.render_formula(
        version="0.6.3",
        sha_x86_mac="x86-mac-sha",
        sha_arm_mac="arm-mac-sha",
        sha_x86_linux="x86-linux-sha",
        sha_arm_linux="arm-linux-sha",
    )

    assert 'version "0.6.3"' in formula
    assert "releases/download/v0.6.3/mumei-x86_64-apple-darwin.tar.gz" in formula


def test_install_script_platform_assets_exist_in_release_matrix() -> None:
    install_script = INSTALL_SCRIPT.read_text()
    matrix_names = release_matrix_names()

    assert "mumei-${arch}-${os}" in install_script
    assert "${platform}.tar.gz" in install_script
    assert re.search(
        r"Linux\)\s+# Plan 7: Prefer musl.*?x86_64.*?os=\"unknown-linux-musl\"",
        install_script,
        re.DOTALL,
    )
    assert re.search(
        r"arm64\|aarch64\).*?if \[ \"\$os\" = \"unknown-linux-gnu\" \]",
        install_script,
        re.DOTALL,
    )
    for platform in {
        "mumei-x86_64-unknown-linux-musl",
        "mumei-aarch64-unknown-linux-gnu",
        "mumei-x86_64-apple-darwin",
        "mumei-aarch64-apple-darwin",
    }:
        assert platform in matrix_names


def test_release_workflow_publishes_matrix_named_archives() -> None:
    workflow = RELEASE_WORKFLOW.read_text()

    assert "tar czf ../${{ matrix.name }}.tar.gz ." in workflow
    assert "path: ${{ matrix.name }}.tar.gz" in workflow
    assert 'Compress-Archive -Path "release-pkg/*" -DestinationPath "${{ matrix.name }}.zip"' in workflow
    assert "path: ${{ matrix.name }}.zip" in workflow


def test_release_workflow_keeps_musl_and_windows_stabilization() -> None:
    workflow = RELEASE_WORKFLOW.read_text()

    assert "Build release binary (macOS)" in workflow
    assert re.search(
        r"- name: Build release binary \(macOS\)\s+if: runner\.os == 'macOS'\s+run: cargo build --release --target \$\{\{ matrix\.target \}\}\s+",
        workflow,
    )
    assert "rustup toolchain install nightly --profile minimal" in workflow
    assert "cargo +nightly build -Zhost-config -Ztarget-applies-to-host" in workflow
    assert 'printf "host-config = true\\n"' in workflow
    assert 'printf "target-applies-to-host = false\\n\\n"' in workflow
    assert "$maxAttempts = 5" in workflow
    assert "Length -lt 10MB" in workflow
    assert "Length -lt 1MB" in workflow


def test_homebrew_update_is_warning_only_after_release() -> None:
    workflow = RELEASE_WORKFLOW.read_text()

    assert "needs: release" in workflow
    assert "id: homebrew-token" in workflow
    assert "should_update=false" in workflow
    assert "::warning::HOMEBREW_TAP_TOKEN is not set" in workflow
    assert "::warning::HOMEBREW_TAP_TOKEN is invalid" in workflow
    assert "if: steps.homebrew-token.outputs.should_update == 'true'" in workflow
    assert "::error::HOMEBREW_TAP_TOKEN" not in workflow
    assert "if ! git push origin HEAD:main; then" in workflow
