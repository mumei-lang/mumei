# Phase 4c+ Roadmap — Session Plans

These are 8 sequential implementation plans for Phase 4c and beyond.
Each plan is a self-contained session prompt with full context, implementation details, affected files, and acceptance criteria.
Plans should be executed in priority order (Plan 1 first).

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
