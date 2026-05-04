# Claude Code + mumei クイックスタート

## 前提条件

- Claude Code CLI がインストール済み
  - 推奨: `curl -fsSL https://claude.ai/install.sh | bash`
  - npm 環境では `npm install -g @anthropic-ai/claude-code`
- Python 3.11+ と `mcp[cli]>=1.0` がインストール済み
- mumei リポがクローン済み、`cargo build` が成功する状態

## セットアップ

1. `pip install "mcp[cli]>=1.0"`
2. mumei リポのルートで `claude` を起動
3. `.mcp.json` が project scope の MCP 設定として自動検出され、`mumei-forge` MCP サーバーが stdio transport で起動する
4. Claude Code 内で `/mcp` を実行し、`mumei-forge` の状態を確認して必要なツール使用を許可する

## `.mcp.json` について

Claude Code はリポジトリルートの `.mcp.json` を共有プロジェクト設定として読み込む。mumei では `mcp_server.py` をリポジトリルートから起動する必要があるため、設定は `sh -lc` で明示的に `cd` してから Python サーバーを起動する。

```json
{
  "mcpServers": {
    "mumei-forge": {
      "command": "sh",
      "args": ["-lc", "cd . && exec python mcp_server.py"]
    }
  }
}
```

Claude Code のドキュメントでは project scope の設定は `.mcp.json` に保存される。標準例は `command` / `args` / `env` を中心にしており、`cwd` は Claude Code の一部バージョンで無視されることがあるため、互換性の高い shell wrapper 形式を使う。

## 動作確認

Claude Code 内で以下を試す:

- 「std/ の健全度を測定して」→ `measure_std_health` が呼ばれる
- 「std/core.mm の証明証明書を見せて」→ `get_proof_certificate` が呼ばれる
- 「以下の mumei コードを検証して: ...」→ `validate_logic` が呼ばれる
- 「std/ の依存グラフを可視化して」→ `visualize_std_graph` が呼ばれる

## 典型的なワークフロー: バグのある `.mm` ファイルの修正

1. Claude Code に `.mm` ファイルを見せる
2. `validate_logic` で検証エラーを確認
3. Claude が `semantic_feedback` を読み取り、修正案を生成
4. 修正後のコードを再度 `validate_logic` で検証
5. 成功したら `forge_blade` でビルド

## MCP ツール早見表

| Tool | 目的 |
| --- | --- |
| `validate_logic(source_code)` | Z3 検証のみ。修正ループで使う |
| `forge_blade(source_code, output_name)` | 検証 + LLVM IR 生成 |
| `execute_mm(source_code, output_name, command)` | `build` / `verify` / `check` の実行 |
| `get_inferred_effects(source_code)` | コード生成前にエフェクト要件を確認 |
| `get_allowed_effects(project_dir)` | 現在のエフェクト境界を確認 |
| `set_allowed_effects(allowed, denied)` | エフェクト境界を動的に変更 |
| `list_std_catalog()` | std/ の検証済みコンポーネント一覧 |
| `analyze_std_gaps()` | std/ の欠落コンポーネント分析 |
| `visualize_std_graph(format)` | std/ 依存グラフ可視化 |
| `measure_std_health()` | std/ の健全度メトリクス |
| `get_proof_certificate(module_path)` | 証明証明書の取得 |
| `generate_doc(source_code, format)` | 構造化ドキュメント生成 |
