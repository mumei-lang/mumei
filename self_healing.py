import subprocess
import json
import os
import re
import time
from openai import OpenAI
from dotenv import load_dotenv

# .envファイルから環境変数を読み込む
load_dotenv()

# LLMプロバイダー設定（Qwen3.5 / Ollama / vLLM / OpenAI 対応）
api_key = os.getenv("LLM_API_KEY", os.getenv("OPENAI_API_KEY", ""))
base_url = os.getenv("LLM_BASE_URL", None)  # None の場合は OpenAI デフォルト
model = os.getenv("LLM_MODEL", "gpt-4o")

if not api_key:
    raise ValueError(
        "LLM_API_KEY (または OPENAI_API_KEY) が設定されていません。"
        ".env ファイルを確認してください。"
    )

# OpenAI互換クライアントの初期化（Ollama / vLLM / 外部API も対応）
client_kwargs = {"api_key": api_key}
if base_url:
    client_kwargs["base_url"] = base_url

client = OpenAI(**client_kwargs)

SOURCE_FILE = "sword_test.mm"
OUTPUT_BASE = "katana"
REPORT_FILE = "report.json"  # output_dir (カレントディレクトリ) に合わせる
MAX_RETRIES = 5  # 修正回数の上限

def run_mumei():
    """コンパイラを実行。exit(1)があれば正常に失敗を検知する"""
    result = subprocess.run(
        ["cargo", "run", "--", "build", SOURCE_FILE, "-o", OUTPUT_BASE],
        capture_output=True, text=True
    )
    # returncodeが0以外なら失敗
    return result.returncode == 0, result.stdout + result.stderr

def get_fix_from_ai(source_code, error_log, report_data):
    """AIにエラー内容と検証レポート（反例）を送り、修正案を取得する"""
    prompt = f"""
あなたはMumei言語の専門家です。以下のコードは形式検証に失敗しました。
特に 'requires' (事前条件) を修正して、数学的矛盾を解消してください。

# ソースコード:
{source_code}

# エラーログ:
{error_log}

# 検証レポート (反例データ):
{json.dumps(report_data, indent=2)}

修正後のコードのみを、```rust ... ``` の形式で出力してください。
"""
    response = client.chat.completions.create(
        model=model,
        messages=[{"role": "system", "content": "You are a helpful programming assistant."},
                  {"role": "user", "content": prompt}]
    )

    content = response.choices[0].message.content or ""
    # コードブロック部分のみ抽出（Qwen3.5 の ```Rust / ```rs 等にも対応）
    code_match = re.search(r'```(?:rust|rs|Rust)\s*\n(.*?)```', content, re.DOTALL)
    if code_match:
        return code_match.group(1).strip()
    # フォールバック: コードブロックが見つからない場合はそのまま返す
    return content.strip()

def main():
    print("Mumei Self-Healing Loop Start...")

    for attempt in range(MAX_RETRIES):
        success, logs = run_mumei()

        if success:
            print(f"Success! Blade is flawless (Attempt {attempt + 1}).")

            return

        print(f"Attempt {attempt + 1}: Flaw detected. Consulting AI...")

        # 最新の検証レポートを読み込む
        try:
            with open(REPORT_FILE, "r") as f:
                report = json.load(f)
        except Exception:
            report = {"status": "error", "reason": "Report not found"}

        with open(SOURCE_FILE, "r") as f:
            source = f.read()

        # AIから修正コードを取得
        fixed_code = get_fix_from_ai(source, logs, report)

        # ファイルを書き換え
        with open(SOURCE_FILE, "w") as f:
            f.write(fixed_code)

        print("Code updated. Retrying...")
        time.sleep(2)

    print("Healing failed. The blade remains broken.")

if __name__ == "__main__":
    main()
