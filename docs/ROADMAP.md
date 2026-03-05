# 🗺️ Strategic Roadmap — Mumei v0.3.0+

> Mumei を「実験的言語」から「実用ツール」へ昇華させる 3 つの戦略的ロードマップ

## Overview

| Priority | Theme | Goal | Status |
|---|---|---|---|
| 🥇 P1 | Network-First Standard Library | API スクリプティング言語としての実用性 | 🔧 In Progress |
| 🥈 P2 | Runtime Portability | どこでも動く配布基盤 | 📋 Planned |
| 🥉 P3 | CLI Developer Experience | 世界最高の CLI 開発体験 | 📋 Planned |

---

## 🥇 Priority 1: Network-First Standard Library

### Vision

現代のプログラミングにおいて HTTP リクエストと JSON 操作は「標準装備」であるべきです。
PR #29 の FFI 基盤を活用し、**「Rust のパワーを Mumei の皮で包む」** 実装を最優先します。

**狙い**: 「API を叩いてデータを加工するスクリプト」を Mumei で書く動機を作ります。

### Phase A: FFI Bridge Completion

extern 宣言から trusted atom への自動変換を完成させます。
これが std.http / std.json の **前提条件** です。

**Current State**:
- ✅ `extern "Rust" { fn sqrt(x: f64) -> f64; }` 構文パース済み
- ✅ `ExternFn` / `ExternBlock` AST + Span 付き
- ✅ `Item::ExternBlock` 全 match 網羅
- ❌ extern → ModuleEnv 自動登録 (trusted atom)
- ❌ LLVM コード生成 (extern 関数の declare + call)

**Implementation Plan**:

```
1. ExternBlock → trusted atom 自動変換
   - ExternFn のシグネチャから Atom を生成
   - TrustLevel::Trusted を設定（body 検証スキップ）
   - ModuleEnv.atoms に自動登録

2. LLVM declare 生成
   - extern 関数を LLVM IR の `declare` として出力
   - 型マッピング: Mumei 型 → LLVM 型

3. 呼び出し側コード生成
   - ModuleEnv に登録された extern atom への call 生成
   - ABI 互換性の確保 (extern "C" / extern "Rust")
```

**Files to modify**:
- `src/main.rs` — `load_and_prepare()` で ExternBlock → atom 変換
- `src/verification.rs` — extern atom の trusted 検証
- `src/codegen.rs` — LLVM `declare` + `call` 生成
- `docs/FFI.md` — 実装ステータス更新

### Phase B: std.json

文字列とオブジェクトの相互変換。Mumei の型推論と組み合わせて型安全に JSON を扱えるようにします。

**Target API**:

```mumei
import "std/json" as json;

// Parse: string → structured data
let data = json.parse(raw_string);

// Stringify: structured data → string
let output = json.stringify(data);

// Type-safe field access
let name = json.get_string(data, "name");
let age = json.get_int(data, "age");
```

**Backend**: `serde_json` (既に Cargo.toml に依存済み)

**Files to create/modify**:
- `std/json.mm` — JSON 操作の atom 定義
- `src/parser.rs` — 文字列リテラル型の拡張（必要に応じて）
- `docs/STDLIB.md` — std.json リファレンス追加

### Phase C: std.http (Client)

`reqwest` を FFI バックエンドに隠蔽した HTTP クライアント。

**Target API**:

```mumei
import "std/http" as http;

// Simple GET — 極限のシンプルさ
let response = await http.get("https://api.example.com/users");
let status = http.status(response);
let body = http.body(response);

// POST with JSON body
let response = await http.post("https://api.example.com/users", payload);
```

**Backend**: Rust `reqwest` crate (FFI 経由)

**Files to create/modify**:
- `std/http.mm` — HTTP 操作の atom 定義
- `Cargo.toml` — `reqwest` 依存追加
- `docs/STDLIB.md` — std.http リファレンス追加

### Phase D: Integration Demo

`task_group` との並行リクエスト統合デモ。

```mumei
import "std/http" as http;
import "std/json" as json;

// Concurrent API requests — Mumei's killer feature
task_group:all {
    task { http.get("https://api.example.com/users") };
    task { http.get("https://api.example.com/orders") };
    task { http.get("https://api.example.com/products") }
}
```

**Files to create**:
- `examples/http_demo.mm` — HTTP デモ
- `examples/json_demo.mm` — JSON デモ
- `examples/concurrent_http.mm` — 並行 HTTP デモ

---

## 🥈 Priority 2: Runtime Portability

### Vision

「どこでも動く」ことは普及の絶対条件です。
導入のハードルをゼロに近づけ、GitHub Actions や CI/CD 環境での
「ちょっとした自動化スクリプト」の座を狙います。

### Phase A: Static Linking Optimization

依存する共有ライブラリを全て静的にリンクし、
`mumei` 実行ファイル一つあればどこでも動く状態を完璧にします。

**Current State**:
- ✅ GitHub Actions release workflow (macOS x86_64/aarch64, Linux x86_64)
- ✅ `mumei setup` で Z3/LLVM 自動ダウンロード
- ❌ musl ターゲット (完全静的リンク)
- ❌ Windows バイナリ

**Implementation Plan**:

```
1. musl ターゲット追加
   - x86_64-unknown-linux-musl ターゲット
   - GitHub Actions に musl ビルドジョブ追加

2. 依存ライブラリの静的リンク確認
   - Z3: 静的リンク可能か検証
   - LLVM: 静的リンク設定の確認
   - 全ターゲットで ldd 検証

3. Windows サポート (stretch goal)
   - x86_64-pc-windows-msvc ターゲット
   - GitHub Actions に Windows ジョブ追加
```

**Files to modify**:
- `.github/workflows/release.yml` — musl/Windows ビルド追加
- `Cargo.toml` — 静的リンク設定
- `docs/TOOLCHAIN.md` — サポートプラットフォーム更新

### Phase B: Homebrew Tap

`brew install mumei-lang/mumei` で一発導入。

**Implementation Plan**:

```
1. mumei-lang/homebrew-mumei リポジトリ作成
2. Formula 作成 (GitHub Releases からダウンロード)
3. CI で Formula の自動更新 (release.yml 連携)
```

**Formula example**:
```ruby
class Mumei < Formula
  desc "Mathematical Proof-Driven Programming Language"
  homepage "https://github.com/mumei-lang/mumei"
  url "https://github.com/mumei-lang/mumei/releases/download/v0.3.0/mumei-aarch64-apple-darwin.tar.gz"
  sha256 "..."
  license "MIT"

  def install
    bin.install "mumei"
    (share/"mumei-std").install Dir["std/*"]
  end
end
```

### Phase C: WebInstall (curl | sh)

```bash
curl -fsSL https://mumei-lang.github.io/install.sh | sh
```

**Implementation Plan**:

```
1. install.sh スクリプト作成
   - OS/arch 自動検出
   - GitHub Releases から最新バイナリをダウンロード
   - PATH への追加案内

2. GitHub Pages でホスティング
3. README にインストール手順追加
```

**Files to create**:
- `scripts/install.sh` — インストールスクリプト
- `.github/workflows/release.yml` — install.sh の自動更新

---

## 🥉 Priority 3: CLI Developer Experience

### Vision

LSP に注力しない分、「CLI 上での開発体験」を世界最高レベルにします。
ドキュメントが充実している言語は、ユーザーが自走できるため、
コミュニティが勝手に育ち始めます。

### Phase A: mumei repl

構文を試せる REPL (Read-Eval-Print Loop) を強化し、
HTTP リクエストなどを試せるようにします。

**Target UX**:

```
$ mumei repl
Mumei v0.3.0 REPL — type :help for commands, :quit to exit

mumei> type Nat = i64 where v >= 0;
Type defined: Nat

mumei> atom inc(n: Nat) requires: n >= 0; ensures: result >= 1; body: n + 1;
✅ Verified: inc

mumei> inc(5)
= 6

mumei> inc(-1)
❌ Verification failed: requires n >= 0, but got n = -1

mumei> :load examples/http_demo.mm
Loaded 3 atoms from examples/http_demo.mm

mumei> :quit
```

**Implementation Plan**:

```
1. REPL ループ基盤
   - rustyline (行編集 + 履歴) or 標準入力ベース
   - parse → verify → eval のパイプライン

2. インクリメンタル定義
   - ModuleEnv への逐次追加
   - 定義の上書き対応

3. 特殊コマンド
   - :help, :quit, :load, :env (現在の定義一覧)
   - :type <expr> (型推論結果表示)

4. HTTP/JSON 統合 (P1 完了後)
   - REPL から http.get() を直接実行
```

**Files to create/modify**:
- `src/repl.rs` — REPL エンジン
- `src/main.rs` — `mumei repl` サブコマンド追加
- `Cargo.toml` — `rustyline` 依存追加

### Phase B: mumei doc

ソースコード内のコメントから、Rust の `rustdoc` のように
綺麗な HTML ドキュメントを生成する機能。

**Target UX**:

```bash
$ mumei doc src/main.mm -o docs/

# Generates:
# docs/index.html
# docs/atoms/increment.html   (requires/ensures/body)
# docs/types/Nat.html          (refinement predicate)
# docs/traits/Comparable.html  (methods + laws)
```

**Doc comment syntax**:

```mumei
/// Increments a natural number by 1.
///
/// # Examples
/// ```
/// inc(5) == 6
/// inc(0) == 1
/// ```
atom inc(n: Nat)
    requires: n >= 0;
    ensures: result >= 1;
    body: n + 1;
```

**Implementation Plan**:

```
1. Doc comment パーサー
   - /// コメントの抽出
   - Markdown パース (簡易版)

2. HTML テンプレートエンジン
   - atom / type / trait / struct / enum 各ページ
   - インデックスページ (全定義の一覧)
   - requires/ensures の可視化

3. CSS スタイリング
   - ダークモード対応
   - シンタックスハイライト

4. CLI 統合
   - mumei doc <input> -o <output_dir>
   - mumei doc --json (構造化出力)
```

**Files to create/modify**:
- `src/doc.rs` — ドキュメント生成エンジン
- `src/main.rs` — `mumei doc` サブコマンド追加
- `templates/` — HTML テンプレート

### Phase C: REPL + HTTP Integration

REPL から直接 HTTP リクエストを試せるデモ (P1 + P3A 完了後)。

```
mumei> import "std/http" as http;
mumei> let res = await http.get("https://httpbin.org/get");
mumei> http.status(res)
= 200
mumei> http.body(res)
= "{ \"origin\": \"...\" }"
```

---

## Dependencies

```
P1-A (FFI Bridge) ──→ P1-B (std.json) ──→ P1-D (Integration Demo)
                  ──→ P1-C (std.http)  ──→ P1-D
                                        ──→ P3-C (REPL + HTTP)

P2-A (Static Link) ──→ P2-B (Homebrew) ──→ P2-C (WebInstall)

P3-A (REPL) ─────────→ P3-C (REPL + HTTP)
P3-B (mumei doc)       (independent)
```

---

## Success Metrics

| Metric | Target | Measurement |
|---|---|---|
| **API Script Demo** | `http.get` + `json.parse` が動作 | examples/http_demo.mm が通る |
| **Install Time** | < 30 seconds | `curl \| sh` from clean environment |
| **REPL Responsiveness** | < 100ms per eval | Benchmark on standard hardware |
| **Doc Coverage** | 100% of std library | `mumei doc std/` generates all pages |
| **Binary Size** | < 50MB (static) | `ls -la target/release/mumei` |
| **Platform Support** | macOS + Linux + Windows | CI green on all targets |

---

## Timeline (Estimated)

| Phase | Duration | Milestone |
|---|---|---|
| P1-A: FFI Bridge | 1-2 weeks | extern → trusted atom 自動登録 |
| P1-B: std.json | 1 week | `json.parse` / `json.stringify` |
| P1-C: std.http | 1-2 weeks | `http.get` / `http.post` |
| P1-D: Demo | 1 week | 統合デモ + ドキュメント |
| P2-A: Static Link | 1 week | musl ビルド + CI |
| P2-B: Homebrew | 1 week | `brew install mumei` |
| P2-C: WebInstall | 1 week | `curl \| sh` |
| P3-A: REPL | 2 weeks | `mumei repl` 基本動作 |
| P3-B: Doc Gen | 2-3 weeks | `mumei doc` HTML 生成 |
| P3-C: Integration | 1 week | REPL + HTTP 統合 |

---

## Related Documents

- [`docs/FFI.md`](FFI.md) — FFI extern block design (Phase A foundation)
- [`docs/CONCURRENCY.md`](CONCURRENCY.md) — Structured concurrency (Phase D foundation)
- [`docs/STDLIB.md`](STDLIB.md) — Standard library reference (Phase B/C additions)
- [`docs/TOOLCHAIN.md`](TOOLCHAIN.md) — CLI commands and distribution
- [`instruction.md`](../instruction.md) — Development guidelines and priorities
