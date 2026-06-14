# Mumei (無銘) [![GitHub](https://img.shields.io/github/stars/mumei-lang/mumei?style=social)](https://github.com/mumei-lang/mumei)

[English](README.md)

**`.mm`を書く前に、既存コードと仕様を形式手法で検証します。**

Mumei は、既存の外部言語コード（たとえば Python、Rust、Go、TypeScript）、自然言語要件、または Mumei `.mm` モジュールから開始できる形式検証ツールチェーンです。Z3、proof certificate、AI-agent ワークフローを使ってバグ、仕様ドリフト、矛盾を見つけ、重要なロジックを数学的に検査された `.mm` コードへ段階的に移行する道筋を提供します。

[Technical Paper](paper/) — proof-driven programming architecture、自律検証ループ、case study。

> existing code / natural language spec → MCP or mumei-agent → Z3-backed diagnostics → optional `.mm` migration → LLVM / proof artifacts

---

## `.mm`を書かずに始める（mumei-agent）

mumei-agent を使うと、既存コードや仕様をそのまま検証できます。
`.mm` を書かない入口から段階的に `.mm` へ移行する手順は [Onboarding Guide](docs/ONBOARDING.md) を参照してください。

### インストール

```bash
git clone https://github.com/mumei-lang/mumei-agent
cd mumei-agent
cp .env.example .env  # Set LLM_BASE_URL / LLM_API_KEY / LLM_MODEL
uv sync
# After this, run commands as uv run mumei-agent <subcommand>
```

### 3つのユースケース

**1. 既存コードのバグ候補を見つける**
```bash
uv run mumei-agent validate-code --input src/payment.py --language python  # --language is required: python|rust|go
```

**2. 仕様↔コードのドリフトを検出する**
```bash
uv run mumei-agent validate-spec-to-code --spec docs/spec.txt --code src/payment.py
```

**3. 仕様単体の矛盾を見つける**
```bash
uv run mumei-agent validate-spec --input docs/spec.txt --format nl  # --format is optional (default: nl)
```

`--domain financial` などのドメインヒントは任意です。単一ファイルだけでなくディレクトリも指定できます（例: `--input src/`）。

詳細は [Verification Workflow Guide](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/VERIFICATION_WORKFLOW_GUIDE.md) を参照してください。

## 自己修復ループ: `.mm`を書かずに始める

自然言語仕様検証、外部コード検証、仕様↔コード整合性を含む完全な no-`.mm` ワークフローについては、mumei-agent の [Verification Workflow Guide](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/VERIFICATION_WORKFLOW_GUIDE.md) を参照してください。
`mumei-agent` のソース checkout から作業する場合は、一度 `uv sync` を実行します。その後は同じコマンドを `uv run mumei-agent <subcommand>` として利用できます。

### 1. 既存コード: バグ候補箇所を見つける

既存のソースファイルを agent に渡し、contract の推論、検証、疑わしいパスの報告を依頼します。`--input` は必須で、単一のソースファイルを指します。`--language` も必須で、`python`、`rust`、`go` のいずれかである必要があります。

```bash
mumei-agent validate-code --input src/payment.py --language python  # --language is required: python|rust|go
```

MCP agent は、`.mm` を合成または受け取った後、mumei の検証 backend を直接利用できます。

```json
{
  "tool": "validate_logic",
  "arguments": {
    "source_code": "atom debit(balance: i64, amount: i64) requires: amount > 0; ensures: result >= 0; body: balance - amount;"
  }
}
```

### 2. 自然言語仕様 + 既存コード: 仕様↔コードのドリフトを検出する

何かを `.mm` に移行する前に、要件と実装を比較して不一致を検出します。`--spec` と `--code` は必須です。`--code` は単一のソースファイルを指します。`--language` は任意で、`python`、`rust`、`go` を指定できます。

```bash
mumei-agent validate-spec-to-code --spec docs/spec.txt --code src/payment.py
```

逆方向のドリフト検出:

```bash
mumei-agent validate-code-to-spec \
  --code src/payment.py \
  --spec docs/spec.txt \
  --language python  # Optional: python|rust|go
```

### 3. 仕様のみ: 矛盾と未規定の振る舞いを見つける

文章の要件から開始し、直接的な矛盾、vacuity、曖昧さ、過剰制約を確認します。`--input` は必須です。ドメイン固有のヒントが必要な場合、`--domain` は任意で指定できます。

```bash
mumei-agent validate-spec --input docs/spec.txt --domain payment  # --domain is optional
```

MCP agent は、入力が文章、抽出済み contract、`.mm` のどれであるかに応じて、spec-health tool または verification tool を呼び出せます。

```json
{
  "tool": "forge_blade",
  "arguments": {
    "source_code": "atom safe_div(a: i64, b: i64) requires: b != 0; ensures: true; body: a / b;",
    "output_name": "safe_div"
  }
}
```

---

## 段階的な移行パス

### Step 0: MCP または mumei-agent で既存資産を検証する

まず既存コードと仕様に対して agent を実行します。`.mm` source は不要です。

```bash
mumei-agent validate-code --input src/payment.py --language python
mumei-agent validate-spec-to-code --spec spec.txt --code src/payment.py --language python
```

得られた counter-example、drift report、提案 contract を移行 backlog として利用します。

### Step 1: 重要な仕様面を `.mm` で書き始める

最小の高リスク contract を `.mm` atom に変換し、CLI または MCP で検証します。

```bash
mumei verify specs/payment.mm
```

```json
{
  "tool": "validate_logic",
  "arguments": {
    "source_code": "atom transfer(balance: i64, amount: i64) requires: balance >= amount && amount > 0; ensures: result >= 0; body: balance - amount;"
  }
}
```

### Step 2: 新しい検証済みコードを `.mm` で書く

core contract が安定したら、新しい logic を直接 `.mm` で実装し、実行可能 artifact を出力します。

```bash
mumei build src/main.mm -o dist/output
mumei run src/main.mm
```

---

## インストール

```bash
# One-liner (macOS / Linux)
curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash

# Homebrew
brew install mumei-lang/mumei/mumei

# Specific version (latest is v0.6.0)
curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash -s -- --version v0.6.0
```

過去のバージョンと changelog については [Releases](https://github.com/mumei-lang/mumei/releases) を参照してください。

Rust toolchain は不要です。OS/arch は自動検出されます。

<details>
<summary>ソースからビルド</summary>

```bash
# macOS
brew install llvm@17 z3
# Linux
sudo apt-get install -y libz3-dev llvm-17-dev libclang-17-dev

cargo build --release   # -> target/release/mumei
cargo install --path .  # -> ~/.cargo/bin/mumei

# Or auto-install Z3/LLVM
mumei setup && source ~/.mumei/env
```

</details>

---

## ツールリファレンス

### CLI

| コマンド | 説明 |
|---------|------|
| `mumei build <file> -o <out>` | 検証 + codegen（`--emit llvm-ir`（デフォルト）/ `c-header` / `verified-json` / `proof-book` / `decidable-metrics` / `proof-cert` / `escalation-bundle` / `binary` / `rust` / `python` / external plugin name） |
| `mumei run <file>` | 検証 → codegen → link → `atom main()` を native binary として実行（`--emit binary` がデフォルト、`--emit llvm-ir` は link 前に IR を保持） |
| `mumei verify <file>` | Z3 検証のみ |
| `mumei check <file>` | parse + resolve（高速、Z3 なし） |
| `mumei init <name>` | project template を生成 |
| `mumei add <dep>` | dependency（path / git / registry）を追加 |
| `mumei publish` | local registry に公開 |
| `mumei list` | local registry の利用可能 package を一覧表示 |
| `mumei setup` | Z3 + LLVM toolchain を download |
| `mumei inspect` | development environment を表示 |
| `mumei infer-effects <file>` | 必要な effect を推論（JSON output） |
| `mumei infer-contracts <file>` | すべての atom の contract を推論（JSON output） |
| `mumei repl` | interactive REPL |
| `mumei doc <file> -o <dir>` | documentation を生成（`--format html`（デフォルト）/ `markdown` / `json`） |
| `mumei lsp` | LSP server を起動 |
| `mumei verify-cert <cert> <file>` | 現在の source に対して proof certificate を検証 |

### MCP Tools

| Tool | 説明 |
|------|------|
| `forge_blade` | 検証 + code generation を1ステップで実行 |
| `validate_logic` | Z3 検証のみ。counter-example と semantic feedback data を返す |
| `execute_mm` | 汎用 build / check 実行 |
| `get_inferred_effects` | 事前 check: コードを書く前に必要な effect を推論 |
| `get_allowed_effects` | session の現在の effect boundary を問い合わせる |
| `set_allowed_effects` | effect boundary を動的に override |
| `analyze_std_gaps` | std/ coverage の gap を特定 |
| `list_std_catalog` | std/ catalog 内のすべての atom を一覧表示 |
| `visualize_std_graph` | std/ dependency graph を描画（Mermaid または DOT） |
| `measure_std_health` | std/ health metrics を測定 |
| `get_proof_certificate` | module の proof certificate を取得 |
| `generate_doc` | structured documentation を生成（`mumei doc --format json`） |
| `analyze_contract_conflicts` | cross-atom contract conflict と circular dependency を解析（Meta-Architect） |
| `propose_interface_refactoring` | architecture issue に対する interface-level refactoring を提案（Meta-Architect） |
| `get_spec_guideline` | agent 向け spec-writing guideline を JSON として返す |
| `get_structured_feedback` | source code に対する P9-E structured feedback JSON を返す |

### プロジェクト構成

```text
mumei/
├── mumei-core/             # Core library: parser, HIR, verification, MIR, emitter trait
├── mumei-emit-llvm/        # LLVM IR emitter (LlvmEmitter + codegen)
├── mumei-emit-json/        # Verified JSON metadata emitter (VerifiedJsonEmitter)
├── mumei-emit-proofbook/   # Markdown proof-certificate emitter
├── mumei-emit-rust/        # Rust FFI binding emitter
├── mumei-emit-python/      # Python FFI binding emitter
├── mumei-ffi-tests/        # Generated Rust property tests for FFI contracts
├── src/                    # CLI binary (main.rs, cli.rs, lsp.rs, setup.rs)
├── std/                    # Standard library (.mm files)
├── runtime/                # C runtime library (mumei_runtime.c)
├── visualizer/             # std/ dependency graph generation scripts
├── scripts/                # Install script, utility scripts (install.sh, etc.)
├── benchmarks/             # Dafny-style and SV-COMP-style benchmarks
├── paper/                  # Technical paper
├── editors/vscode/         # VS Code extension (LSP client + counter-example decorations)
├── examples/               # Example programs
└── tests/                  # Integration tests (.mm files)
```

---

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
| [LSP Integration](docs/LSP_INTEGRATION.md) | Editor CodeLens、intent drift、spec-code mapping |
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

---

## ライセンス

[Apache-2.0 license](LICENSE)
