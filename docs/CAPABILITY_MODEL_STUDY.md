# Object-Based Capability Model 設計調査（P19 / Priority 15）

> 調査日: 2026-08-30。対象は `docs/CAPABILITY_SECURITY.md` Section 3 の
> "Object-Based Capability Model (Alternative)" を、現行の parameterized effect system
> （Option A: effects + Z3）と互換を保ったまま導入できるかどうかの**非破壊な設計調査**。
> 本ドキュメントは調査成果物であり、コンパイラの実装は一切含まない。

## 0. スコープと結論サマリ

| 調査項目 | 影響範囲 | opt-in 判定基準（`grant` 未使用の既存 `.mm` が現行セマンティクスのまま通る） |
|---|---|---|
| 1. 新 AST ノードの要否 | 新規 `Item` / `Expr` / `HirExpr` / `Rvalue` の追加のみ。既存ノードの意味は不変 | ✅ 充足（ただし字句解析はコンテキスト依存キーワードで導入すること） |
| 2. 型システム拡張 | 新しい型コンストラクタ `cap<E>` と constraint implication による subtyping | ✅ 充足（`cap` 型を持たないプログラムには新規則が発火しない） |
| 3. Z3 エンコーディング | 既存 `check_constant_constraint()` / `parse_constraint_to_z3_string()` の再利用 | ✅ 充足（static capability に限る場合。value-dependent constraint は対象外） |
| 4. ランタイム表現の要否 | 静的解決できる範囲では compile-time に完全消去可能 | ✅ 充足（capability を struct フィールド / 配列 / 戻り値に載せない範囲） |

**総合結論: 肯定的 — 実装フェーズに進める。** ただし「最小サブセット」を厳密に切ることが条件で、
下記 §6 に挙げる 3 つの拡張（value-dependent constraint、capability の data structure への格納、
dynamic dispatch を要する capability 選択）は本調査の範囲外であり、これらを最初から取り込むと
zero runtime overhead と Z3 の決定可能な断片の両方を壊す。最小サブセットに限れば、
effect containment 証明（`UsedEffects(body) ⊆ AllowedEffects(signature)`）と effect propagation checking は
**規則そのものを変えずに**（capability 型パラメータを effect 名の新しい供給源として扱うだけで）維持できる。
Option A は最小サブセットが入るまでの既定パスとして残り、`grant` を使わないコードでは恒久的に既定のままとなる。

調査の根拠となる現行実装は以下:

| 参照先 | 内容 |
|---|---|
| `mumei-core/src/parser/ast.rs` | `Effect` / `EffectDef` / `EffectParam` / `Expr` / `Item` / `Atom` / `Param` |
| `mumei-core/src/parser/lexer.rs` | キーワード表（識別子文字列からトークンへの無条件マッピング） |
| `mumei-core/src/ast.rs` | `TypeRef.effect_set`、エフェクト多相の単相化（`effects: [E]` → `effects: [FileWrite]`） |
| `mumei-core/src/hir.rs` | `HirEffectSet` / `HirEffectUsage` / `HirExpr::Perform` |
| `mumei-core/src/mir.rs` | `Rvalue::Perform`、`Movability` と `movability_from_type()` |
| `mumei-core/src/mir_analysis/move_analysis.rs` | 前方データフローの move 解析（use-after-move / double-move） |
| `mumei-core/src/verification/module_env.rs` | `LinearityCtx`（Z3 レベルの borrow / consume 追跡） |
| `mumei-core/src/verification/support/effects.rs` | `verify_effect_containment()` / `verify_effect_params()` / `check_constant_constraint()` / `parse_constraint_to_z3_string()` |
| `mumei-emit-llvm/src/codegen/expr_emit.rs` | `perform E.op(args)` → `__effect_{E}_{op}` への直接呼び出し |

想定する表層構文は `docs/CAPABILITY_SECURITY.md` Section 3 のものをそのまま使う:

```mumei
type FileCap = capability SafeFileRead(path: Str) where starts_with(path, "/tmp/");

atom read_config(cap: FileCap, filename: Str)
    requires: starts_with(filename, "/tmp/");
    ensures: result >= 0;
    body: { perform cap.read(filename); 0 };

atom main()
    effects: [SafeFileRead(path)]
    requires: true;
    ensures: result >= 0;
    body: {
        let cap = grant SafeFileRead where starts_with(path, "/tmp/");
        read_config(cap, "/tmp/config.txt")
    };
```

---

## 1. 新 AST ノードの要否

### 1.1 追加が必要なノード

| 層 | 追加内容 | 既存要素との関係 |
|---|---|---|
| `parser/ast.rs` | `Item::CapabilityDef(CapabilityDef)`（`name` / `effect_name` / `params: Vec<EffectDefParam>` / `constraint: Option<String>` / `span`） | `EffectDef` とフィールド構成がほぼ同一。`capability` 宣言は「既存 `EffectDef` に別名と constraint を与える view」であり、新しい effect を定義しない |
| `parser/ast.rs` | `Expr::Grant { effect: String, constraint: Option<String>, span }` | `Expr::Perform` と同じ位置づけの新バリアント。`perform` は「効果を起こす」、`grant` は「効果を起こす権利を値にする」 |
| `parser/ast.rs` | narrowing は新ノードを追加せず `Expr::Grant` の再適用（`grant cap where <constraint>`）で表現できる | 別ノードにすると型規則が二重化するため非推奨 |
| `parser/ast.rs` | `Expr::Perform` の `effect: String` を「effect 名 または capability 変数名」として解決する必要がある（フィールド追加ではなく解決フェーズの分岐） | 既存 `.mm` では常に effect 名に解決されるため意味は不変 |
| `hir.rs` | `HirExpr::Grant`、および `HirEffectUsage` に「どの capability 変数由来か」を持たせる任意フィールド | `HirEffectSet` の構造自体は不変 |
| `mir.rs` | `Rvalue::Grant { effect, constraint }` | `Rvalue::Perform` と同格。move 解析の対象になる（§2） |

capability 型そのものは `TypeRef` で表現でき、新しい型ノードは不要である。`TypeRef` は
すでに `effect_set: Option<Vec<String>>` を持ち、`atom_ref(i64) -> i64 with [FileWrite]` の
エフェクト情報を運んでいる。`cap: FileCap` は `TypeRef { name: "FileCap", type_args: [], effect_set: Some(vec!["SafeFileRead"]) }`
として表せるため、capability パラメータは既存の「効果付き関数型パラメータ」とまったく同じ形で
署名に現れる。これが §3 の containment 保存の鍵になる。

### 1.2 影響範囲の実測

新しい `Expr` バリアントは exhaustive `match` を持つすべての箇所にアームを足す必要がある。
現行の `Expr::Perform` を参照している Rust ファイルは 17（`mumei-core/src/parser/expr.rs`、
`mumei-core/src/ast.rs`、`mumei-core/src/hir.rs`、`mumei-core/src/mir.rs`、
`verification/translator/expr.rs`、`verification/executor.rs`、`verification/fragment.rs`、
`verification/vacuity.rs`、`verification/spurious_detection.rs`、`verification/loop_detector.rs`、
`verification/support/{call_graph,dataflow_inference,resource_safety,task_ownership}.rs`、
`src/codegen.rs`、`mumei-emit-llvm/src/codegen/expr_emit.rs`、`mumei-emit-llvm/src/binary.rs`）、
`HirExpr::Perform` が 6 ファイル、`Rvalue::Perform` が 6 ファイルである。`Grant` を追加する場合の
機械的な作業量はこれと同規模で、**追加はすべて新アームであり既存アームの書き換えを伴わない**。

### 1.3 既存 `effect` 宣言との共存

- `capability` 宣言は `EffectDef` を消費するだけで、新しい effect を定義しない。したがって
  `ModuleEnv::effect_defs` / `resolve_leaf_effects()` / `is_subeffect()` のエフェクト階層は不変で、
  合成 effect（`IO includes: [FileRead, FileWrite, Console]`）の解決も変わらない。
- `perform cap.read(path)` は、`cap` の静的型が指す effect 名へ解決したうえで、
  既存の `perform SafeFileRead.read(path)` と同一の検証経路（`EffectCtx::perform_effect()` →
  `verify_effect_containment()`）に入る。
- 逆方向の共存（capability 宣言のない effect を `grant` する）は許してよい。`grant SafeFileRead where ...`
  は無名の capability 型を生成するだけである。

### 1.4 唯一の破壊リスク: 字句解析

`parser/lexer.rs` のキーワード表は識別子文字列を無条件にトークンへ写像している
（`"perform" => Token::Perform` など）。ここに `"grant"` / `"capability"` を素朴に足すと、
`grant` や `capability` を atom 名・変数名・struct 名として使っている既存ソースが
**パース段階で壊れる**。リポジトリ内の `.mm`（`std/`、`examples/`、`tests/`）では
`grant` / `capability` は識別子として使われておらず（出現はコメントのみ）、in-tree の回帰は起きないが、
外部ソースは保証できない。したがって導入時は次のいずれかを取る:

1. **コンテキスト依存キーワード（推奨）**: `grant` は `let x = ` の直後、`capability` は
   `type X = ` の直後でのみキーワードとして解釈し、それ以外の位置では `Token::Ident` のままにする。
2. 既存トークンの再利用: `capability` を導入せず `type FileCap = effect SafeFileRead where ...;`
   と綴る（`Token::Effect` / `Token::Where` は既存）。`grant` のみが新語彙になる。

**判定基準の充足**: ✅ 充足。追加ノードはすべて新バリアントであり、既存 `Item` / `Expr` /
`HirExpr` / `Rvalue` の意味論は変わらない。`grant` を含まないソースは新しい構文経路に一切入らない。
ただし充足は「コンテキスト依存キーワードとして導入すること」を前提とする。無条件キーワード追加は
（in-tree では無害でも）外部ソースに対して破壊的であり、opt-in 要件に反する。

---

## 2. 型システム拡張（subtyping と linearity）

### 2.1 capability の型と subtyping

capability 値の型を `cap<E, C>`（`E` = effect 名、`C` = constraint 式）とすると、
自然な subtyping は**制約の含意**である:

```
C1 ⟹ C2                        E1 = E2 または is_subeffect(E1, E2)
------------------------------------------------------------------
                  cap<E1, C1>  <:  cap<E2, C2>
```

- 右側（effect 階層）は既存 `ModuleEnv::is_subeffect()` をそのまま使える。新しい階層は増えない。
- 左側（constraint の含意）は「狭い capability を広い capability の位置に渡せる」という
  narrowing の本体で、`starts_with(path, "/tmp/config/") ⟹ starts_with(path, "/tmp/")` のような判定になる。
  これは §3 のとおり既存の Z3 String Sort 断片で表現でき、`Solver::check()` 1 回で判定できる
  （`¬(C1 ⟹ C2)` が unsat なら subtype）。
- これは `docs/CAPABILITY_SECURITY.md` §2.3 / §2.4 が「`requires` 契約による暗黙の narrowing」と
  呼んでいるものを、値の型に明示的に載せ替えたものにすぎない。証明義務の総量は増えない。

### 2.2 linearity との相互作用 — revocation の実装候補としての move 追跡

「渡した capability は呼び出し元で使えない」= capability をアフィン値として扱う、という要件は、
**既存の move 解析にほぼ無改造で載る**。

- `mir.rs` の `movability_from_type()` は、`i64` / `f64` / `bool` と一部の refined type を `Copy`、
  **それ以外の未知の型名をすべて `Move`** として分類する。したがって `cap: FileCap` の local は
  追加実装なしで `Movability::Move` になる。
- `mir_analysis/move_analysis.rs` の前方データフローは、`Move` local の `Use` を消費として扱い、
  消費後の使用を use-after-move、二重消費を double-move として報告する。
  分岐 join では `MirLinearityState::merge()` が「片方の経路でのみ消費された」局面を
  `MergeConflict` として検出する。これは capability の条件付き委譲（`if c { f(cap) } else { g() }` の後に
  `cap` を使う）をそのまま検出できることを意味する。
- `LinearityCtx`（`verification/module_env.rs`）は Plan 19 以降 move 検出の主経路ではなく、
  Z3 レベルの borrow / consume 追跡として残っている。したがって revocation の一次実装は
  MIR 側に置くのが正しく、`LinearityCtx` は「借用中の capability は consume できない」という
  補助規則（`borrow()` / `consume()` の既存ロジック）を再利用する二次的な位置づけになる。
- 明示構文が要る場合も、`consume cap;` は既存の `Token::Consume` / `Atom.consumed_params` で表現でき、
  新しい構文要素は不要である。

**限界**: この方式で得られるのは「移動による失効」（アフィン性）であり、
「発行済み capability を後から一斉に無効化する」という動的 revocation ではない。
後者はランタイムの間接参照と失効フラグを要するため zero runtime overhead を壊す（§4）。
最小サブセットでは move ベースの失効のみを対象とし、動的 revocation は非対象とする。

**判定基準の充足**: ✅ 充足。`cap` 型が現れないプログラムでは新しい subtyping 規則は一度も発火せず、
move 解析の分類（`movability_from_type()`）も既存の型名に対しては現状のまま。
`Copy` 型の扱いにも変更はないため、既存の所有権診断の結果は不変である。

---

## 3. Z3 エンコーディング

### 3.1 capability 値に載る制約の表現

現行の制約検証は 2 経路に分かれている（`verification/support/effects.rs`）:

- 定数引数: `check_constant_constraint(value, constraint)` が Rust 側で
  `starts_with` / `contains` / `ends_with` / `not_contains` / `matches`（regex crate）を直接評価する。
- 変数引数: `parse_constraint_to_z3_string()` が同じ語彙を Z3 String Sort の
  `str.prefixof` / `str.suffixof` / `str.contains` とその否定へ写像し、`matches` は
  `^p.*` / `.*s$` / `.*sub.*` / `^lit$` / `^p.*s$` の各形を prefix / suffix / contains / eq に近似する。

capability の constraint はこの**まったく同じ文字列断片**である（`capability` 宣言の `where` 句は
`EffectDef.constraint` と同じ文法）。したがって新しい制約言語も新しい Z3 sort も不要で、
必要なのは「制約をどこから引くか」の変更だけになる:

| 現行 | capability 導入後 |
|---|---|
| `verify_effect_params()` が `effect.name` → `ModuleEnv.effect_defs[name].constraint` を引く | `perform cap.op(x)` では `cap` の静的型 `cap<E, C>` の `C` を引く。`cap` が `grant E where C'` 由来なら `C'`、パラメータなら宣言型の `C` |

subtyping の判定（§2.1）も同じ断片で閉じる: `C1 ⟹ C2` は、
`parse_constraint_to_z3_string()` で得た 2 つの `Bool` について `Bool::and(&[C1, ¬C2])` を assert し、
`SatResult::Unsat` を確認すればよい。`Unknown`（タイムアウト）は既存 `verify_effect_params()` と同じく
警告扱いにするか、より安全側に倒して narrowing を拒否する。近似できない複雑な regex では
`parse_constraint_to_z3_string()` が `None` を返すため、その場合は「定数引数のみ許可」に落とすのが妥当である。

### 3.2 effect containment / propagation を壊さないこと

`verify_effect_containment()` は次の 3 つの規則で構成されている（`support/effects.rs`）:

1. 呼び出し先の leaf effect 集合 ⊆ 呼び出し元の leaf effect 集合（合成 effect は
   `resolve_leaf_effects_from_effects()` で解決してから比較）。
2. 否定 effect（`!IO`）の禁止チェック。
3. **関数型パラメータの `effect_set` ⊆ 呼び出し元の leaf effect 集合**。

capability パラメータはこの 3 番目の規則の既存形にそのまま乗る。`cap: FileCap` は
`TypeRef.effect_set = Some(["SafeFileRead"])` を持つパラメータであり、既存コードが
`type_ref.is_fn_type()` で絞っている条件を「関数型 **または** capability 型」に広げるだけで、
比較式（`param_leaves ⊆ allowed_leaves`、`is_subeffect` によるフォールバック）は一字も変えずに済む。

- `read_config(cap: FileCap, ...)` は `effects:` に `SafeFileRead` を書かなくても、
  capability 型から effect を要求していることが署名に現れる。呼び出し元 `main` は
  `effects: [SafeFileRead(path)]` を宣言していなければ規則 3 で弾かれる。
  つまり **propagation checking の不等式は変わらず、effect 名の供給源が 1 つ増えるだけ**である。
- `grant E where C` は「その atom が E を行使する権利を作る」ので、`grant` を含む atom の
  `UsedEffects(body)` に `E` を加える。これも既存 `EffectCtx::perform_effect()` と同じ経路で、
  宣言集合に含まれなければ既存の effect violation として報告される。
- 新しい verdict 語彙・新しい失敗分類は不要である。capability 由来の失敗はすべて既存の
  effect violation / effect propagation violation として `verification_violations` / `next_steps` に載る。

### 3.3 対象外とすべきもの

**value-dependent constraint**（実行時の値に依存して制約が決まる capability、
例: `grant FileRead where starts_with(path, user_home(uid))`）は、制約が閉じた文字列リテラルでなくなるため
`parse_constraint_to_z3_string()` の断片から外れ、量化を含む文字列制約（一般には決定不能）に落ちる。
最小サブセットでは constraint を「コンパイル時に確定する閉じた式」に限定する。

**判定基準の充足**: ✅ 充足。既存 `.mm` は capability 型パラメータも `grant` も持たないため、
`verify_effect_params()` の引き方も `verify_effect_containment()` の 3 規則も現状の入力に対して同一に動く。
検証結果の報告経路と語彙も変わらない。

---

## 4. ランタイム表現の要否

### 4.1 現行の perform のコード生成

`mumei-emit-llvm/src/codegen/expr_emit.rs` は `HirExpr::Perform { effect, operation, args }` を
`__effect_{effect}_{operation}` という**名前で直接解決される関数呼び出し**に落とす。
effect ハンドラのディスパッチテーブルも effect 記述子オブジェクトも実行時には存在しない。
エフェクト多相（`<E: Effect>` + `with E`）も、`mumei-core/src/ast.rs` の単相化で
`effects: [E]` → `effects: [FileWrite]` に置換されてから codegen に渡るため、
すでに「型レベルの effect 抽象を compile-time に消去する」前例が存在する。

### 4.2 capability を消去できる条件

`perform cap.op(args)` を `__effect_{E}_{op}(args)` に落とせるのは、
`cap` の指す effect 名が**その perform 地点で静的に一意に決まる**場合である。以下の範囲なら常に成立する:

- `let cap = grant E where C;` による束縛（右辺で `E` が確定）
- capability 型パラメータ（宣言型で `E` が確定）
- 上記の narrowing（`grant cap where C'` は `E` を変えず `C` を狭めるだけ）

この範囲では capability 値はランタイム表現を持たず、`Rvalue::Grant` は codegen で消える
（値を生成しない）。constraint は §3 の検証で消費され、実行時には残らない。
**現行の zero runtime overhead は完全に維持される。**

### 4.3 消去できなくなるケース

- capability を struct フィールド / 配列要素 / 戻り値に格納する: 値が制御フローをまたいで運ばれ、
  effect 名がフロー依存になるため、タグ付き表現と間接呼び出しが必要になる。
- 異なる effect の capability を分岐で選ぶ（`let c = if b { capA } else { capB }`）: 同上。
- 動的 revocation: 失効フラグを実行時に参照する必要があり、原理的に消去できない。

いずれも最小サブセットから除外することで回避できる。除外は型規則で強制できる
（capability 型は struct フィールド型 / 配列要素型 / 戻り型に出現できない、という制限を課す）。

**判定基準の充足**: ✅ 充足。`grant` を含まないソースには capability 値が存在せず、
codegen 経路（`__effect_*` 直接呼び出し）も生成物も現状と同一である。
ランタイム（`runtime/mumei_runtime.c`）への追加も不要。

---

## 5. 契約・語彙への影響

なし。本調査は `harness_contract` / `intent_fidelity` / `artifact_paths` /
`budget_policy_fingerprint` / `lean_verified` および no-`.mm` の 8 キーのいずれにも触れない。
実装フェーズに進む場合も、capability 由来の検証結果は既存の effect 検証と同じ経路
（`verification_status` / `verification_violations` / `next_steps` と proof certificate）で報告し、
新しい verdict 分類・別名 alias は追加しない（§3.2）。

---

## 6. 実装フェーズに進む場合の段階分割案

調査結果が肯定的であることを受けた提案であり、着手判断は別途行う。

| Stage | 内容 | 完了条件 |
|---|---|---|
| Stage 1 | `capability` 型宣言（コンテキスト依存キーワード）+ capability 型パラメータのみ。`grant` なし | 既存 effect と等価な検証結果になること。`.mm` 回帰ゼロ |
| Stage 2 | `grant E where C` 式と静的 capability 束縛。codegen は消去のみ | `grant` を含む新規テストが通り、既存 codegen 出力がバイト単位で不変 |
| Stage 3 | narrowing（`grant cap where C'`）と `C1 ⟹ C2` の Z3 判定 | narrowing の受理 / 拒否が Z3 で判定でき、`Unknown` 時の安全側動作が定義されている |
| Stage 4 | move ベースの revocation（アフィン capability） | use-after-move / double-move が capability に対して報告される |

非対象（本調査の前提を壊すため、必要になった時点で改めて調査する）:
value-dependent constraint、capability の data structure への格納、
分岐による capability の動的選択、動的 revocation。

---

## 7. 総合結論

**実装フェーズに進んでよい（肯定的）。** 4 調査項目すべてで opt-in 判定基準
「`grant` を使わない既存 `.mm` が現行セマンティクスのまま通る」が充足され、
破壊的変更が不可避である証拠は見つからなかった。決め手は次の 3 点である:

1. capability パラメータは `TypeRef.effect_set` を持つパラメータとして表現でき、
   effect containment / propagation の不等式を**書き換えずに**再利用できる（§3.2）。
2. capability の constraint は既存の文字列制約断片と同一で、新しい Z3 sort も
   新しい制約言語も不要（§3.1）。
3. `movability_from_type()` が未知の型名を `Move` に分類するため、
   アフィンな capability（move による失効）は MIR move 解析にほぼ無改造で載る（§2.2）。

一方で、Section 3 の "Disadvantages" が挙げる「ランタイム表現が必要」「破壊的変更」は、
最小サブセット（静的 capability・閉じた constraint・データ構造に載せない）に限れば**回避できる**ことが
本調査で確認できた。逆に、この境界を越える機能（§6 非対象）を初回から取り込むと、
zero runtime overhead と Z3 の決定可能断片の両方を失うため、その場合は
Option A（parameterized effects + Z3）継続が正しい判断となる。

したがって `docs/CAPABILITY_SECURITY.md` §4 の Recommendation は現時点で撤回しない。
Option A は既定パスのままとし、Stage 1〜4 が opt-in 拡張として上積みされる、という位置づけとする。
