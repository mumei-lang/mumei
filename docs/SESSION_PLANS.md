# Phase 4c+ Roadmap — Session Plans

These are sequential implementation plans for Phase 4c and beyond.
Each plan is a self-contained session prompt with full context, implementation details, affected files, and acceptance criteria.
Plans should be executed in priority order (Plan 1 first).

> **Status**: Plans 1–8 completed. Plans 9–11 completed in PR (Plans 9-11). Plans 15–20 completed in PR #83.

---

## Plan 1: Phase 4c — MIR Copy/Move 型区別の統合

### 目的

MIR の Move Analysis において、Copy 型（Int, Nat, Pos, Bool, f64 および標準ライブラリの refined type: RawPtr, NullablePtr, HumanAge）と Move 型を区別し、
Copy 型に対する false positive を排除した上で、Move 違反を warning から hard error に昇格する。

### 背景

`src/verification.rs` の Phase 1h で、全ての `Rvalue::Use(Place::Local(..))` が
move として扱われている。Int/Bool/f64 などの Copy 型でも UseAfterMove 警告が出るため、
全ての違反が warning 止まりになっている。

`src/mir_analysis.rs` の `MirLinearityState` は `HashMap<Local, bool>` で
alive/consumed を追跡しているが、型情報を持っていない。

`src/mir.rs` の `LocalDecl` には `ty: Option<String>` フィールドが既にあるが、
MIR lowering 時に型情報が十分に伝搬されていない。

### 実装内容

1. **`src/mir.rs` の `LocalDecl` に型情報を確実に伝搬する**
   - `alloc_local()` の呼び出し元で、パラメータの型名を渡す
   - `alloc_temp()` で生成される一時変数にも型情報を付与

2. **`src/mir_analysis.rs` の `process_statement_for_moves()` を修正**
   - Copy 型の判定ヘルパー関数 `fn lookup_movability(local, locals) -> Movability` を追加（`movability_from_type()` を参照）
   - Copy 型の場合は `consume()` をスキップ

3. **`src/verification.rs` の Phase 1h を修正**
   - Copy 型でない場合は hard error に昇格
   - Copy 型の場合はスキップ

4. **テスト追加**
   - Copy 型変数の再利用が violation にならないテスト
   - Move 型の UseAfterMove が正しく検出されるテスト

### 対象ファイル

- `src/mir.rs`: LocalDecl, Movability, movability_from_type, alloc_local, lower_expr
- `src/mir_analysis.rs`: lookup_movability, process_statement_for_moves
- `src/verification.rs`: Phase 1h warning → error 昇格

### 受け入れ基準

- `cargo test` が全て通る
- Copy 型の変数を複数回使用しても warning/error が出ない
- Move 型の UseAfterMove は hard error になる
- `cargo fmt` 適用済み

---

## Plan 2: MIR Lowering — 残り式形式の対応

### 目的

MIR lowering でプレースホルダー定数（`Int(0)`）に変換されている式形式を、
適切な MIR 構造に lowering する。

### 背景

`src/mir.rs` で、HirExpr::Match, AtomRef, CallRef, Async, Await, Task, TaskGroup, Lambda が
プレースホルダーに変換されていた。

### 実装内容

1. **HirExpr::Match** → SwitchInt ベースの CFG に変換
2. **HirExpr::AtomRef** → `MirConstant::FuncRef(String)` を追加
3. **HirExpr::CallRef** → callee を評価し `Rvalue::Call` に変換
4. **HirExpr::Lambda** → captures + body を inline で lowering
5. **HirExpr::Async / Await / Task / TaskGroup** → body を lower_stmt で処理

### 対象ファイル

- `src/mir.rs`: lower_expr 関数の各ケース
- `src/mir.rs`: MirConstant に FuncRef variant を追加

### 受け入れ基準

- プレースホルダーが全て適切な lowering に置き換わる
- `cargo test` が全て通る
- Match を含む atom の MIR dump で SwitchInt が生成される
- `cargo fmt` 適用済み

---

## Plan 3: MIR 制御フロー lowering の堅牢化

### 目的

if/else と while-loop の MIR lowering で、ネストされた制御フローに対して
BasicBlock ID の算術計算が off-by-one になる脆弱性を修正する。

### 背景

if/else lowering では次のブロック ID を算術的に予測しているが、
ネストされた制御フローで内部ブロックが追加されると ID がずれる。

### 実装内容

1. **`LowerCtx` にブロック予約メカニズムを追加**
   - `patch_terminator(&mut self, block_id, terminator)` メソッドを追加
2. **if/else lowering のリファクタリング** — placeholder → patch パターン
3. **while-loop lowering のリファクタリング** — 同様に動的 ID 決定
4. **テスト追加** — 3段以上ネストした制御フローのテスト

### 対象ファイル

- `src/mir.rs`: LowerCtx, lower_expr (IfThenElse), lower_stmt (While)

### 受け入れ基準

- ネストした制御フローで正しい CFG が生成される
- 既存テストが全て通る
- `cargo fmt` 適用済み

---

## Plan 4: MIR Drop Insertion — SwitchInt discriminant の後続ブロック Drop 挿入

### 目的

SwitchInt の discriminant にのみ使われる変数が、後続ブロックで drop されず
リソースリークが発生する問題を修正する。

### 背景

`src/mir_analysis.rs` の TODO: SwitchInt discriminant 変数は terminator で
使用されるため terminator 前に drop できないが、後続ブロックでの compensating drop が未実装。

### 実装内容

1. **`insert_drops` に第2パスを追加** — SwitchInt の successor に Drop を挿入
2. **ヘルパー関数の追加**
   - `fn terminator_used_locals(terminator) -> HashSet<Local>`
   - `fn block_already_drops(block, local) -> bool`
3. **テスト追加** — match/if-else の discriminant drop 検証

### 対象ファイル

- `src/mir_analysis.rs`: `insert_drops` 関数の拡張

### 受け入れ基準

- SwitchInt discriminant 変数が全 successor ブロックで drop される
- 二重 drop が発生しない
- 既存テストが全て通る
- `cargo fmt` 適用済み

---

## Plan 5: Z3 String Sort マイグレーション

### 目的

エフェクトパラメータの文字列制約検証を、Z3 の native String sort に移行する。

### 背景

定数パスは Rust 側で直接評価、変数パスは Symbolic String ID で処理されていた。
Z3 String sort を使うことで、変数パスの制約も正確に検証可能になる。

### 実装内容

1. **Z3 String sort 制約生成** — `starts_with` → `str.prefixof`, `contains` → `str.contains`, `ends_with` → `str.suffixof`
2. **Sort-aware timeout** — String sort 使用時に timeout を2倍に設定
3. **VCtx の `has_string_constraints` フラグ** を有効化
4. **テスト追加** — 定数/変数パスの各制約検証

### 対象ファイル

- `src/verification.rs`: verify_effect_params, VCtx, Sort-aware timeout
- `Cargo.toml`: z3 crate バージョン確認

### 受け入れ基準

- 変数パスの文字列制約が Z3 String sort で検証される
- 定数パスの Constant Folding は引き続き動作する
- 典型的な制約が 500ms 以内に解決される
- `cargo test` が全て通る
- `cargo fmt` 適用済み

---

## Plan 6: Effect Hierarchy 拡張

### 目的

エフェクトシステムに以下の4つの拡張を実装する:
1. Effect Aliases
2. Multi-parent (Intersection)
3. Effect narrowing
4. Negative effects

### 背景

`EffectDef` は `parent: Vec<String>` で多親をサポート（`Option<String>` から変更済み）。
エフェクト階層の解決は `get_effect_ancestors()`（BFS）と `is_subeffect()` で行われている。

### 実装内容

1. **Effect Aliases** — `effect IO = FileRead | FileWrite;` 構文
2. **Multi-parent** — `parent: [Network, Encrypted]` 構文、`EffectDef.parent` を `Vec<String>` に変更
3. **Effect Narrowing** — callee の declared effects と caller の available effects の交差計算
4. **Negative Effects** — `effects: [!IO]` 構文、body 内の forbidden effect 検出

### 対象ファイル

- `src/parser/ast.rs`: EffectDef.parent の型変更, Effect.negated 追加
- `src/parser/item.rs`: effect 定義パーサー, effects リストパーサー
- `src/parser/token.rs`: Pipe トークン追加
- `src/verification.rs`: get_effect_ancestors, is_subeffect, verify_effect_consistency

### 受け入れ基準

- Effect aliases がパースされ展開される
- Multi-parent がパースされ、サブタイプ関係が正しい
- Negative effects が body 内の forbidden effect を検出する
- 既存テストが全て通る
- `cargo fmt` 適用済み

---

## Plan 7: Runtime Portability — musl Static Linking + Windows バイナリ

### 目的

GitHub Actions のリリースワークフローに musl ターゲット（完全静的リンク）と
Windows バイナリのビルドジョブを追加する。

### 背景

現在 macOS (Intel/ARM) と Linux (gnu) のみビルドされている。
musl と Windows が未対応としてマークされている。

### 実装内容

1. **musl ターゲット** — `x86_64-unknown-linux-musl` を matrix に追加
2. **Windows ターゲット** — `x86_64-pc-windows-msvc` を matrix に追加
3. **パッケージングの修正** — Windows は `.zip`、musl は静的リンク検証
4. **install.sh の更新** — musl バイナリの自動検出

### 対象ファイル

- `.github/workflows/release.yml`: matrix 拡張
- `scripts/install.sh`: musl/Windows 対応

### 受け入れ基準

- musl ターゲットがリリースワークフローに含まれる
- Windows ターゲットがリリースワークフローに含まれる
- install.sh が musl 環境を検出する
- `cargo fmt` 適用済み

---

## Plan 8: Concurrency 改善 — Task Return Types, TaskGroup Binding, Cancellation, Channels

### 目的

concurrency 機能の以下の未実装項目を実装する:
1. Task return type inference
2. task_group 結果のバインディング構文
3. Task cancellation semantics
4. Channel 型 (`chan<T>`)

### 背景

`instruction.md` に TODO として記載されていた4つの concurrency 改善項目。

### 実装内容

1. **Task Return Type Inference** — 型チェッカーで `task { expr }` ブロックの最終式から戻り型を推論
2. **TaskGroup Result Binding** — `let results = task_group { ... }` 構文
3. **Task Cancellation Semantics** — `cancel group_name;` 文、cancellation token メカニズム
4. **Channel Type (`chan<T>`)** — `send(ch, value)` / `recv(ch)` 操作、`Chan` エフェクト

### 対象ファイル

- `src/parser/token.rs`: Chan, Send, Recv, Cancel トークン
- `src/parser/lexer.rs`: キーワード認識
- `src/parser/ast.rs`: ChanSend, ChanRecv, Cancel variants
- `src/parser/expr.rs`: send/recv パーサー、cancel 文
- `src/hir.rs`: HirExpr::ChanSend, HirExpr::ChanRecv, HIR lowering
- `src/ast.rs`: Monomorphizer 更新
- `src/codegen.rs`: LLVM codegen
- `src/mir.rs`: MIR lowering
- `src/transpiler/rust.rs`, `golang.rs`, `typescript.rs`: 各言語トランスパイラ
- `src/verification.rs`: Z3 検証、各種 collect 関数
- `instruction.md`: セマンティクス文書化

### 受け入れ基準

- task の戻り型が正しく推論される
- task_group の結果がバインド可能
- cancellation が動作する
- `chan<T>` で send/recv が可能
- `cargo test` が全て通る
- `cargo fmt` 適用済み

---

## Plan 9: Enum Payload 型の正確な LLVM 型解決

### 目的

codegen.rs のハードコードされた i64 ペイロードスロットを、EnumVariant.field_types に基づく正しい LLVM 型（f64, ptr for Str, struct for enums）に修正する。

### 背景

`enum_llvm_type()` は全てのペイロードスロットを `context.i64_type()` で生成していた。
Str フィールドを持つ enum バリアントは `ptr` 型を使う必要があり、f64 フィールドは `f64_type()` を使う必要がある。

### 実装内容

1. **`enum_llvm_type()` (L37-81):** `module_env` パラメータを追加し、`resolve_param_type()` を使用して各スロット位置の実際のフィールド型を解決。バリアント間で型が異なる場合は最大互換型を使用。
2. **Call-site の更新:** `enum_llvm_type()` の全呼び出し箇所に `Some(module_env)` を渡すよう更新。
3. **StructInit field types (L860-867):** `resolve_param_type()` を使用した一貫した型解決に変更。

### 対象ファイル

- `src/codegen.rs`: enum_llvm_type(), StructInit field types

### 受け入れ基準

- `cargo test` が全て通る（201 tests）
- Str ペイロードフィールドを持つ enum が正しい LLVM IR を生成（ptr type, not i64）
- `cargo fmt` 適用済み

---

## Plan 10: エフェクト検証 Future Unlocks — 動的パス構築検証 + Regex ポリシー

### 目的

Z3 が動的に構築された文字列（連結による）がエフェクト where 句制約を満たすことを検証できるようにし、正規表現ベースの制約サポートを追加する。

### 背景

Perform ハンドラは各引数に対して新しい Z3 String 変数を作成し、Variable 引数のみ `__str_{var_name}` 環境ルックアップで接続していた。
`"/tmp/" + user_id + "/log.txt"` のような動的構築文字列は、`starts_with(path, "/tmp/")` に対して直接検証できなかった。

### 実装内容

1. **Perform handler (L6250-6285):** `arg_z3_values[i]` が Z3 String Sort を持つ場合、直接使用。新しい非接続変数の作成を回避。
2. **`parse_constraint_to_z3_string()` (L2330-2369):** `matches()` サポートを追加。Z3 String の prefix/suffix/contains による正規表現パターン近似。
3. **`check_constant_constraint()` (L4091-4098):** Rust regex crate による `matches()` サポートを追加。
4. **テストファイル:** `tests/test_dynamic_path_verification.mm`, `tests/test_regex_constraint.mm` を追加。
5. **ドキュメント:** `docs/CAPABILITY_SECURITY.md` を更新。

### 対象ファイル

- `src/verification.rs`: Perform handler, parse_constraint_to_z3_string, check_constant_constraint
- `tests/test_dynamic_path_verification.mm` (新規)
- `tests/test_regex_constraint.mm` (新規)
- `docs/CAPABILITY_SECURITY.md`

### 受け入れ基準

- `"/tmp/" + var + "/file.txt"` が Z3 により `starts_with(path, "/tmp/")` を満たすことが証明される
- `matches()` が定数・変数文字列パスで動作する
- 既存の starts_with/ends_with/contains/not_contains テストが引き続きパスする
- `cargo test` が全て通る（201 tests）
- `cargo fmt` 適用済み

---

## Plan 11: Deferred ツーリング — mumei inspect --ai + Z3 Proof Certificates

### 目的

AI エージェント向けの構造化 JSON 出力 `mumei inspect --ai` と、検証済み atom の Z3 Proof Certificates を実装する。

### 背景

AI エージェント（MCP サーバー経由）がコードベースの検証状態を構造化データとして取得するには、JSON 形式の検査レポートが必要。
Z3 Proof Certificates は、検証結果の暗号学的検証可能な証拠を提供し、オフラインでの証明妥当性確認を可能にする。

### 実装内容

**Part A: mumei inspect --ai**
1. **`src/inspect.rs` (新規):** InspectReport, AtomReport, EffectReport, EnumReport, StructReport, VerificationSummary 構造体を `#[derive(Serialize)]` で定義。`generate_report()` が ModuleEnv と検証結果から InspectReport を生成。
2. **`src/main.rs`:** inspect サブコマンドを拡張: `mumei inspect <file.mm> [--ai] [--format json|text]`

**Part B: Z3 Proof Certificates**
3. **`src/proof_cert.rs` (新規):** ProofCertificate, AtomCertificate 構造体を Serialize/Deserialize で定義。バージョン、タイムスタンプ、mumei バージョン、Z3 バージョン、ファイルパス、atom ごとの検証結果（z3_check_result, content_hash: SHA-256）を含む。
4. **`src/main.rs`:** `--proof-cert` フラグを verify コマンドに追加。`verify-cert` サブコマンドを追加。
5. **`docs/TOOLCHAIN.md`:** 新しい CLI コマンドのドキュメントを追加。Deferred セクションを更新。

### 対象ファイル

- `src/inspect.rs` (新規): InspectReport 生成
- `src/proof_cert.rs` (新規): ProofCertificate 生成・検証
- `src/main.rs`: inspect/verify/verify-cert サブコマンド
- `docs/TOOLCHAIN.md`: CLI ドキュメント更新

### 受け入れ基準

- `mumei inspect examples/demo.mm --ai` が有効な JSON を stdout に出力
- JSON に全 atom の requires/ensures/effects/verification_status が含まれる
- `mumei verify examples/demo.mm --proof-cert` が .proof.json を生成
- .proof.json に atom ごとの z3_check_result と content_hash が含まれる
- `mumei verify-cert` がソース未変更の atom を "proven" と報告
- `cargo test` が全て通る（201 tests）
- `cargo fmt` 適用済み
