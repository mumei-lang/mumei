import os
import re
import subprocess
import json
import tempfile
from pathlib import Path
from mcp.server.fastmcp import FastMCP
from dotenv import load_dotenv

# 環境変数の読み込み
load_dotenv()

# MCPサーバーの初期化
mcp = FastMCP("Mumei-Forge")

@mcp.tool()
def forge_blade(source_code: str, output_name: str = "katana") -> str:
    """
    Mumeiコードを検証し、Rust/Go/TSコードを生成します。
    検証レポートを含め、すべての一時ファイルは隔離されており並行実行しても安全です。
    """
    root_dir = Path(__file__).parent.absolute()

    # 1. リクエストごとに完全隔離された一時ディレクトリを作成
    with tempfile.TemporaryDirectory() as tmpdir:
        tmp_path = Path(tmpdir)
        source_path = tmp_path / "input.mm"
        source_path.write_text(source_code, encoding="utf-8")

        # 2. コンパイラ実行 (出力先を一時ディレクトリに指定)
        output_base = tmp_path / output_name

        result = subprocess.run(
            ["cargo", "run", "--", "build", str(source_path), "-o", str(output_base)],
            cwd=root_dir,
            capture_output=True,
            text=True
        )

        response_parts = []

        # --- 🔍 隔離されたレポートの読み込み (並行安全の核心) ---
        report_file = tmp_path / "report.json"
        if report_file.exists():
            report_data = report_file.read_text(encoding="utf-8")
            response_parts.append(f"### 🔍 検証レポート (Verification Report)\n```json\n{report_data}\n```")

        if result.returncode == 0:
            response_parts.insert(0, f"✅ 鍛造成功: '{output_name}'")
            # 成果物の収集
            for ext in [".rs", ".go", ".ts", ".ll"]:
                gen_file = tmp_path / f"{output_name}{ext}"
                if gen_file.exists():
                    # 拡張子に合わせてシンタックスハイライトを変更
                    lang = "rust" if ext in [".rs", ".ll"] else "go" if ext == ".go" else "typescript"
                    content = gen_file.read_text(encoding="utf-8")
                    response_parts.append(f"\n### 生成コード: {output_name}{ext}\n```{lang}\n{content}\n```")

            return "\n".join(response_parts)
        else:
            # 失敗時：論理欠陥の証拠（レポート）とエラーログをセットで返す
            response_parts.insert(0, f"❌ 鍛造失敗: 論理的な欠陥が証明されました。")
            if result.stderr:
                response_parts.append(f"\n### エラー詳細\n{result.stderr}")

            return "\n".join(response_parts)

@mcp.tool()
def self_heal_loop() -> str:
    """
    self_healing.py を実行し、AIによる自律修正ループ（sword_test.mm対象）を開始します。
    """
    root_dir = Path(__file__).parent.absolute()

    try:
        result = subprocess.run(
            ["python", "self_healing.py"],
            cwd=root_dir,
            capture_output=True,
            text=True,
            timeout=300
        )
        if result.returncode == 0:
            return f"✅ 自律修正完了:\n{result.stdout}"
        else:
            return f"❌ 自律修正失敗:\n{result.stderr}\n{result.stdout}"
    except subprocess.TimeoutExpired:
        return "❌ エラー: 自律修正ループがタイムアウトしました（300秒）。"
    except Exception as e:
        return f"❌ 実行エラー: {str(e)}"

@mcp.tool()
def validate_logic(source_code: str) -> str:
    """
    Mumeiコードの形式検証（Z3）のみを実行します。
    コード生成は行わず、検証結果と反例（Counter-example）を返します。
    AIが .mm コードを修正する際の検証ステップとして使用します。
    """
    root_dir = Path(__file__).parent.absolute()

    with tempfile.TemporaryDirectory() as tmpdir:
        tmp_path = Path(tmpdir)
        source_path = tmp_path / "input.mm"
        source_path.write_text(source_code, encoding="utf-8")

        # mumei verify を実行（Z3検証のみ、コード生成なし）
        result = subprocess.run(
            ["cargo", "run", "--", "verify", str(source_path)],
            cwd=root_dir,
            capture_output=True,
            text=True
        )

        response_parts = []

        # report.json の読み込み（反例データ含む）
        report_file = tmp_path / "report.json"
        if report_file.exists():
            report_data = report_file.read_text(encoding="utf-8")
            response_parts.append(
                f"### 検証レポート\n```json\n{report_data}\n```"
            )

        # stderr から Z3 反例情報を抽出
        if result.stderr:
            counterexamples = re.findall(
                r'Counter-example:.*', result.stderr
            )
            if counterexamples:
                response_parts.append("### Z3 反例 (Counter-examples)")
                for ce in counterexamples:
                    response_parts.append(f"- `{ce.strip()}`")

        if result.returncode == 0:
            response_parts.insert(
                0, "検証成功: 論理的欠陥は検出されませんでした。"
            )
        else:
            response_parts.insert(
                0, "検証失敗: 論理的欠陥が検出されました。"
            )
            if result.stderr:
                response_parts.append(
                    f"\n### エラー詳細\n```\n{result.stderr}\n```"
                )

        return "\n".join(response_parts)


@mcp.tool()
def execute_mm(
    source_code: str,
    output_name: str = "katana",
    command: str = "build",
) -> str:
    """
    Mumeiコードをコンパイル・実行します。
    command: "build" (デフォルト) でフルビルド、"verify" で検証のみ、"check" で構文チェックのみ。
    ビルド結果、生成コード、検証レポートを返します。
    """
    root_dir = Path(__file__).parent.absolute()

    with tempfile.TemporaryDirectory() as tmpdir:
        tmp_path = Path(tmpdir)
        source_path = tmp_path / "input.mm"
        source_path.write_text(source_code, encoding="utf-8")

        output_base = tmp_path / output_name

        # コマンドに応じた引数構築
        cmd_args = ["cargo", "run", "--", command, str(source_path)]
        if command == "build":
            cmd_args.extend(["-o", str(output_base)])

        result = subprocess.run(
            cmd_args,
            cwd=root_dir,
            capture_output=True,
            text=True
        )

        response_parts = []

        # report.json の読み込み
        report_file = tmp_path / "report.json"
        if report_file.exists():
            report_data = report_file.read_text(encoding="utf-8")
            response_parts.append(
                f"### 検証レポート\n```json\n{report_data}\n```"
            )

        if result.returncode == 0:
            response_parts.insert(0, f"{command} 成功: '{output_name}'")
            # 成果物の収集
            for ext in [".rs", ".go", ".ts", ".ll"]:
                gen_file = tmp_path / f"{output_name}{ext}"
                if gen_file.exists():
                    lang = (
                        "rust" if ext in [".rs", ".ll"]
                        else "go" if ext == ".go"
                        else "typescript"
                    )
                    content = gen_file.read_text(encoding="utf-8")
                    response_parts.append(
                        f"\n### 生成コード: {output_name}{ext}"
                        f"\n```{lang}\n{content}\n```"
                    )
        else:
            response_parts.insert(0, f"{command} 失敗")
            if result.stderr:
                response_parts.append(
                    f"\n### エラー詳細\n```\n{result.stderr}\n```"
                )
            if result.stdout:
                response_parts.append(
                    f"\n### 標準出力\n```\n{result.stdout}\n```"
                )

        return "\n".join(response_parts)


if __name__ == "__main__":
    mcp.run()
