# Mumei (無銘) [![GitHub](https://img.shields.io/github/stars/mumei-lang/mumei?style=social)](https://github.com/mumei-lang/mumei)

[English](README.md)

**`.mm`を書く前に、既存コードと仕様を形式手法で検証します。**

Mumei は、既存の外部言語コード、自然言語要件、または Mumei `.mm` モジュールから開始できる形式検証ツールチェーンです。Z3、proof certificate、AI-agent ワークフローでバグ、仕様ドリフト、矛盾を見つけ、重要なロジックを検査済みの `.mm` コードへ段階的に移行します。

[Technical Paper](paper/) — proof-driven programming architecture、自律検証ループ、case study。

> existing code / natural language spec → MCP or mumei-agent → Z3-backed diagnostics → optional `.mm` migration → LLVM / proof artifacts

## no-`.mm` の最前面導線

CLI の `uv run mumei-agent audit --code-file ... --auto-migrate --auto-heal` または MCP の `scan_and_fix` を使い、`.mm` 作成を求める前に既存コードを監査します。契約、固定語彙、V1-A〜E、Lean 昇格、PR evidence は [`docs/CROSS_PROJECT_ROADMAP.md`](docs/CROSS_PROJECT_ROADMAP.md)、[`docs/ROADMAP.md`](docs/ROADMAP.md)、[`docs/ONBOARDING.md`](docs/ONBOARDING.md) を参照してください。

最新の標準ライブラリと mumei-lean の同期ポイントは
[Standard Library Reference](docs/STDLIB.md#cross-project-sync-points) を参照してください。

## `.mm`を書かずに始める（mumei-agent）

mumei-agent は、重要な contract を `.mm` へ移行する前に既存コードや仕様を検証します。詳細は [`docs/ONBOARDING.md`](docs/ONBOARDING.md) と mumei-agent の [Verification Workflow Guide](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/VERIFICATION_WORKFLOW_GUIDE.md) を参照してください。

### インストール

```bash
git clone https://github.com/mumei-lang/mumei-agent
cd mumei-agent
cp .env.example .env  # Set LLM_BASE_URL / LLM_API_KEY / LLM_MODEL
uv sync
# After this, run commands as uv run mumei-agent <subcommand>
```

### 3つのユースケース

```bash
uv run mumei-agent validate-code --input src/payment.py
uv run mumei-agent validate-spec-to-code --spec docs/spec.txt --code src/payment.py
uv run mumei-agent validate-spec --input docs/spec.txt --format nl
```

## 段階的な移行パス

1. **Step 0:** `.mm` なしで既存資産を監査する: `uv run mumei-agent audit --code-file src/payment.py --auto-migrate --auto-heal`
2. **Step 1:** 重要な contract を書いて検証する: `mumei verify specs/payment.mm`
3. **Step 2:** 新しい logic を `.mm` で実装する: `mumei build src/main.mm -o dist/output`

MCP の `scan_and_fix` も同じ audit → migrate-suggest → heal ルートを使います。cross-spec artifact vocabulary と移行ガイダンスは [`docs/CROSS_SPEC_GUIDE.md`](docs/CROSS_SPEC_GUIDE.md) を参照してください。

## P9 NLAE Integration

Mumei は 4 リポジトリ NLAE pipeline の Module B (AR) として contract を Z3 obligation に再構築し、self-correction と Lean fidelity check に渡す Loss Vector を出力します。

```text
mumei-agent → generated .mm → mumei → Loss Vector JSON → self-correct → mumei-lean → mumei-demo
```

phase status、artifact、structured feedback field、E2E workflow は [`docs/CROSS_PROJECT_ROADMAP.md`](docs/CROSS_PROJECT_ROADMAP.md) § P9 を参照してください。

## 分散トレーシング (OpenTelemetry)

`cargo build --features otel` と `OTEL_ENABLED=true` により、`mumei verify` は OTLP span を出力し、`TRACEPARENT` を mumei-agent → Rust → Z3 間で伝播します。無効時は zero-cost で、collector がなくても graceful に動作します。詳細と CI coverage は [`docs/ROADMAP.md`](docs/ROADMAP.md) § P15 と [`.github/workflows/otel-tracing.yml`](.github/workflows/otel-tracing.yml) を参照してください。

## インストール

```bash
curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash
brew install mumei-lang/mumei/mumei
curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash -s -- --version v0.6.14
```

過去のバージョンは [Releases](https://github.com/mumei-lang/mumei/releases) を参照してください。Rust toolchain は不要で、OS/arch は自動検出されます。

<details>
<summary>ソースからビルド</summary>

```bash
brew install llvm@17 z3                 # macOS
sudo apt-get install -y libz3-dev llvm-17-dev libclang-17-dev  # Linux
cargo build --release
cargo install --path .
mumei setup && source ~/.mumei/env
```

</details>

## ツールリファレンス

完全な CLI command table は [`docs/TOOLCHAIN.md`](docs/TOOLCHAIN.md)、MCP tools と setup は [`docs/MCP.md`](docs/MCP.md)、project structure は [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) を参照してください。

## ドキュメント

| ドキュメント | 内容 |
|--------------|------|
| [Verification Workflow Guide](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/VERIFICATION_WORKFLOW_GUIDE.md) | 自然言語仕様の検証、外部コード検証、仕様↔コード整合性、人間向け操作ガイド |
| [MCP Integration](docs/MCP.md) | MCP tools、setup、multi-agent collaboration |
| [Language Reference](docs/LANGUAGE.md) | 型、generics、trait、ownership、async |
| [Features](docs/FEATURES.md) | 以前この README に要約していた feature matrix |
| [Standard Library](docs/STDLIB.md) | Option、Result、List、BoundedArray、sort |
| [Examples & Tests](docs/EXAMPLES.md) | 検証 suite、`.mm` code sample、negative test |
| [Architecture](docs/ARCHITECTURE.md) | compiler internals と repository structure |
| [Report Schema](docs/REPORT_SCHEMA.md) | `report.json`、semantic feedback、rich diagnostics JSON |
| [Cross-Spec Verification](docs/CROSS_SPEC_GUIDE.md) | system-wide contract consistency、invariant、dependency cycle |
| [Toolchain](docs/TOOLCHAIN.md) | CLI command、package management、CI/release |
| [Onboarding Guide](docs/ONBOARDING.md) | 既存コードと自然言語から `.mm` へ進む段階的パス |
| [LSP Integration](docs/LSP_INTEGRATION.md) | Editor CodeLens、intent drift、spec-code mapping、`mumei-agent` spec/code diagnostics |
| [Roadmap](docs/ROADMAP.md) | 戦略ロードマップ |
| [Capability Security](docs/CAPABILITY_SECURITY.md) | effect-based capability security evaluation |
| [Changelog](docs/CHANGELOG.md) | release history |
| [Diagnostics](docs/DIAGNOSTICS.md) | multi-span diagnostics、compound constraint decomposition |
| [Meta-Architect](docs/META_ARCHITECT.md) | contract conflict analysis と interface refactoring tool |
| [Plugin Guide](docs/PLUGIN_GUIDE.md) | Emitter plugin development |
| [Proof Certificate](docs/PROOF_CERTIFICATE.md) | proof certificate schema と usage |
| [Spec Guide](docs/SPEC_GUIDE.md) | Z3-decidable fragment のための spec-writing guideline |
| [FFI](docs/FFI.md) | foreign function interface（Rust/C） |
| [Concurrency](docs/CONCURRENCY.md) | Async/await と deadlock-free resource hierarchy |
| [Editors](docs/EDITORS.md) | VS Code と LSP editor integration |
| [Patterns](docs/PATTERNS.md) | design pattern と idiom |
| [Trusted Atoms](docs/TRUSTED_ATOMS.md) | trusted/unverified atom usage |
| [Structured Feedback Schema](docs/STRUCTURED_FEEDBACK_SCHEMA.md) | P9-E structured feedback JSON schema |
| [Cross-Project Roadmap](docs/CROSS_PROJECT_ROADMAP.md) | mumei + mumei-agent ecosystem roadmap |
| [Claude Code Quickstart](docs/CLAUDE_CODE_QUICKSTART.md) | Claude Code users 向け quickstart guide |


## ライセンス

[Apache-2.0 license](LICENSE)
