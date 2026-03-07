# Mumei (無銘)

**Mathematical Proof-Driven Programming Language**

Mumei formally verifies every function with Z3 before compiling to LLVM IR and transpiling to Rust / Go / TypeScript.

> parse → resolve → monomorphize → **verify (Z3)** → codegen (LLVM IR) → transpile

```mumei
type Nat = i64 where v >= 0;

atom increment(n: Nat)
  requires: n >= 0;
  ensures: result >= 1;
  body: n + 1;
```

---

## Install

```bash
# One-liner (macOS / Linux)
curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash

# Homebrew
brew install mumei-lang/mumei/mumei

# Specific version
curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash -s -- --version v0.2.0
```

No Rust toolchain required. Detects OS/arch automatically.

<details>
<summary>Build from source</summary>

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

## Getting Started

```bash
mumei init my_app
cd my_app
mumei build src/main.mm -o dist/output
```

### CLI

| Command | Description |
|---------|-------------|
| `mumei build <file> -o <out>` | Verify + codegen + transpile |
| `mumei verify <file>` | Z3 verification only |
| `mumei check <file>` | Parse + resolve (fast, no Z3) |
| `mumei init <name>` | Generate project template |
| `mumei add <dep>` | Add dependency (path / git / registry) |
| `mumei publish` | Publish to local registry |
| `mumei setup` | Download Z3 + LLVM toolchain |
| `mumei inspect` | Show development environment |
| `mumei lsp` | Start LSP server |

---

## Features

| Category | Highlights |
|----------|-----------|
| **Types** | Refinement types (`i64 where v >= 0`), Structs, Enums (ADT), Generics |
| **Verification** | Pre/postconditions, loop invariants, termination, `forall`/`exists` quantifiers |
| **Traits** | Algebraic laws verified by Z3 (`law reflexive: leq(x, x) == true`) |
| **Ownership** | `ref` / `ref mut` / `consume` with Z3 aliasing prevention |
| **Concurrency** | `async`/`await`, `task_group:all`/`task_group:any`, deadlock-free proof |
| **Safety** | `trusted` / `unverified` atoms, taint analysis, BMC + inductive invariant |
| **Std Library** | Option, Result, List, BoundedArray, Vector, HashMap, sort algorithms |
| **Output** | LLVM IR + Rust + Go + TypeScript transpiler |
| **Tooling** | LSP server, VS Code extension, `mumei.toml` manifest, dependency manager |

### Rich Diagnostics

```
  × Verification Error: Postcondition (ensures) is not satisfied.
   ╭─[examples/basic.mm:5:1]
 4 │   ensures: result > 0;
 5 │   body: x - 1;
   ·   ──────────── verification failed here
 6 │
   ╰────
  help: Check the ensures condition.
```

Powered by [miette](https://crates.io/crates/miette) — source location, underline highlighting, and actionable suggestions on every error.

---

## Self-Healing Loop (AI + Z3)

Mumei の Self-Healing ループは、AI（LLM）と Z3 形式検証を組み合わせて、コードを自動修正します。

### E2E フロー

```
AI が .mm コードを生成
        |
        v
validate_logic (Z3 検証)
        |
   [失敗?] -----> AI が反例を分析 → 修正コードを生成 → 再検証 (ループ)
        |
   [成功!]
        v
execute_mm (フルビルド: LLVM IR + Rust/Go/TypeScript)
```

### デモ: safe_divide の Self-Healing

**Step a.** AI が初期コードを生成（事前条件が不十分）:

```mumei
type Nat = i64 where v >= 0;

atom safe_divide(a: Nat, b: Nat)
  requires: a >= 0;
  ensures: result >= 0;
  body: { a / b };
```

**Step b.** `validate_logic` で Z3 検証 → 失敗:

```
$ mumei verify input.mm

  x Verification Error: Potential division by zero.
  help: requires に除数 != 0 の条件を追加してください

  Verification: 0 passed, 1 failed
```

**Step c-d.** AI が反例（`b = 0` でゼロ除算）を分析し、修正コードを生成:

```mumei
atom safe_divide(a: Nat, b: Nat)
  requires: a >= 0 && b > 0;   // <- 修正: b > 0 追加
  ensures: result >= 0;
  body: { a / b };
```

**Step e.** 再検証 → 成功:

```
$ mumei verify input.mm
  'safe_divide': verified

  Verification passed: 1 item(s) verified
```

**Step f.** フルビルドで Rust / Go / TypeScript を生成:

```
$ mumei build input.mm -o katana

  Blade forged successfully with 1 atoms.
  Done. Created: katana.rs, katana.go, katana.ts
```

### セットアップ

```bash
# 1. Ollama コンテナ起動
docker compose up -d
docker exec mumei-ollama ollama pull qwen3.5

# 2. 環境変数設定
cp .env.example .env
# .env の パターン1 (Ollama) のコメントを外す

# 3. 依存パッケージインストール
pip install -r requirements.txt

# 4. Self-Healing ループ実行
python self_healing.py

# 5. MCP サーバー起動（Claude Desktop 等から利用）
python mcp_server.py
```

### MCP ツール一覧

| Tool | Description |
|------|-------------|
| `forge_blade` | 検証 + コード生成の一括実行 |
| `self_heal_loop` | 自律修正ループの実行 |
| `validate_logic` | Z3 検証のみ（反例データ返却） |
| `execute_mm` | 汎用ビルド / チェック実行 |

---

## Documentation

| Document | Content |
|----------|---------|
| [Language Reference](docs/LANGUAGE.md) | Types, generics, traits, ownership, async |
| [Standard Library](docs/STDLIB.md) | Option, Result, List, BoundedArray, sort |
| [Examples & Tests](docs/EXAMPLES.md) | Verification suite, pattern matching, negative tests |
| [Architecture](docs/ARCHITECTURE.md) | Compiler internals |
| [Toolchain](docs/TOOLCHAIN.md) | CLI commands, package management |
| [Roadmap](docs/ROADMAP.md) | Strategic roadmap |
| [Changelog](docs/CHANGELOG.md) | Release history |

---

## Development

```bash
pip install pre-commit && pre-commit install
cargo test
cargo clippy
```

---

## License

[MIT](LICENSE)
