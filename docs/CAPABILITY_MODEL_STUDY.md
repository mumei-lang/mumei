# Object-Based Capability Model 設計調査（P19 / Priority 15）

> 調査日: 2026-08-30。対象は `docs/CAPABILITY_SECURITY.md` Section 3 の
> "Object-Based Capability Model (Alternative)" を、現行の parameterized effect system
> （Option A: effects + Z3）と互換を保ったまま導入できるかどうかの**非破壊な設計調査**。
> 本ドキュメントは調査成果物であり、コンパイラの実装は一切含まない。

## 0. スコープと結論サマリ

| 調査項目 | 影響範囲 | opt-in 判定基準（`grant` 未使用の既存 `.mm` が現行セマンティクスのまま通る） |
|---|---|---|
| 1. 新 AST ノードの要否 | 新規 `Item` / `Expr` / `HirExpr` / `Rvalue` の追加と、constraint を保持する capability 専用の型フィールド追加。既存ノードの意味は不変 | ✅ 充足（ただし字句解析はコンテキスト依存キーワードで導入すること） |
| 2. 型システム拡張 | 新しい型コンストラクタ `cap<E>` と constraint implication による subtyping | ✅ 充足（`cap` 型を持たないプログラムには新規則が発火しない） |
| 3. Z3 エンコーディング | 既存 `check_constant_constraint()` / `parse_constraint_to_z3_string()` の再利用 | ✅ 充足（static capability に限る場合。value-dependent constraint は対象外） |
| 4. ランタイム表現の要否 | 静的解決できる範囲では compile-time に完全消去可能（ABI レベルのパラメータ消去パスが必要） | ✅ 充足（capability を struct フィールド / 配列 / 戻り値に載せない範囲） |

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

capability 型は `TypeRef` に**そのままは載らない**。`TypeRef` はすでに
`effect_set: Option<Vec<String>>` を持ち `atom_ref(i64) -> i64 with [FileWrite]` の
エフェクト情報を運んでいるので、`cap: FileCap` の effect 部分 `E` は
`TypeRef { name: "FileCap", type_args: [], effect_set: Some(vec!["SafeFileRead"]) }`
で表現でき、この点だけを見れば capability パラメータは既存の「効果付き関数型パラメータ」と
同じ形で署名に現れる（これが §3 の containment 保存の鍵になる）。

しかし `effect_set` は effect 名の列でしかなく、**constraint `C` を保持できない**。
`FileCap` のような名前付き宣言なら `ModuleEnv` 側の `CapabilityDef` を名前で引けば `C` を復元できるが、
無名の `grant E where C` と narrowing 後の capability には引くべき宣言が存在せず、
`C` が失われる。`C` が失われると §2.1 の subtyping（`C1 ⟹ C2`）も
§3.1 の perform 地点での制約検証も成立しない。したがって追加が必要なのは
**`TypeRef` に載る effect 名ではなく、constraint を持つ capability 専用の型表現**である:

| 層 | 追加内容 |
|---|---|
| `mumei-core/src/ast.rs` | `TypeRef` に `capability: Option<CapabilityType>`（`effect: String` / `constraint: Option<String>`）を追加。既存の構築箇所は `None` のままで意味不変 |
| `hir.rs` / `mir.rs` | 同じ `CapabilityType` を local / パラメータの型情報として伝播させる（`HirAtom` のパラメータ型と `LocalDecl` に持たせる） |

`grant` の結果型と narrowing の結果型はこの構造体に直接書き込まれるため、
宣言名に依存せず AST → HIR → MIR を通して `C` が生存する。既存 `.mm` は
`capability` フィールドが常に `None` になるだけで、型表現の意味は変わらない。

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

1. **コンテキスト依存キーワード（推奨）**: `grant` は**式が来るべき位置（prefix 位置）で、
   直後に識別子が続く場合に限って**キーワードとして解釈する。`let` の右辺だけに限定すると
   `Expr::Grant` を一般の式として扱う設計（§1.1）と矛盾し、引数位置や戻り値位置の narrowing
   （`f(grant cap where C')`）がパースできなくなるためである。変数参照位置の `grant`（`grant + 1`、
   `grant(x)` など後続が識別子でない場合）は `Token::Ident` のままとする。`capability` は
   `type X = ` の直後でのみキーワードとする。
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
C1 ⟹ C2                                     E1 = E2
------------------------------------------------------------------
                  cap<E1, C1>  <:  cap<E2, C2>
```

**effect については不変（`E1 = E2`）に限定する**ことが安全上必須である。`is_subeffect(E1, E2)` を
許すと、子 effect（`FileRead`）の capability を親 effect（`IO`）の capability パラメータに渡せてしまい、
受け取った側は `perform cap.write(...)` のように元の grant にない権限を行使できてしまう
（消去後は `__effect_FileWrite_*` への直接呼び出しになるため、実行時に止める手段もない）。
capability は「行使できる権限」を表すので、effect の向きは共変ではなく反変側に働く。
最小サブセットでは effect 不変とし、effect の広げ・狭めは将来の拡張として別途設計する。
（`is_subeffect()` は引き続き `verify_effect_containment()` 内で使う。ここで禁じるのは
**capability 値の subtyping に effect 階層を使うこと**だけである。）

- constraint の含意は「狭い capability を広い capability の位置に渡せる」という
  narrowing の本体で、`starts_with(path, "/tmp/config/") ⟹ starts_with(path, "/tmp/")` のような判定になる。
  これは §3 のとおり既存の Z3 String Sort 断片で表現でき、`Solver::check()` 1 回で判定できる
  （`¬(C1 ⟹ C2)` が unsat なら subtype）。
- これは `docs/CAPABILITY_SECURITY.md` §2.3 / §2.4 が「`requires` 契約による暗黙の narrowing」と
  呼んでいるものを、値の型に明示的に載せ替えたものにすぎない。証明義務の総量は増えない。

### 2.2 linearity との相互作用 — revocation の実装候補としての move 追跡

「渡した capability は呼び出し元で使えない」= capability をアフィン値として扱う、という要件は、
**既存の move 解析の骨格に載るが、呼び出し地点の所有権移動だけは新規実装が要る**。

- `mir.rs` の `movability_from_type()` は、`i64` / `f64` / `bool` と一部の refined type を `Copy`、
  **それ以外の未知の型名をすべて `Move`** として分類する。したがって `cap: FileCap` の local は
  追加実装なしで `Movability::Move` になる。
- `mir_analysis/move_analysis.rs` の前方データフローは、`Move` local の `Use` を消費として扱い、
  消費後の使用を use-after-move、二重消費を double-move として報告する。
  分岐 join では `MirLinearityState::merge()` が「片方の経路でのみ消費された」局面を
  `MergeConflict` として検出する。
- **ただし委譲そのものは現状では消費にならない**。`process_statement_for_moves()` が
  `consume()` を呼ぶのは `Rvalue::Use(Operand::Place(..))` の場合だけで、`Rvalue::Call { args, .. }` の
  アームは各引数の local に対して `check_alive()` しか行わない。つまり `f(cap)` は `cap` を生かしたままにする。
  Stage 4 では呼び出し地点の所有権移動（callee のパラメータ mode ないし capability 型に基づいて
  引数 local を `consume()` する）を新たに実装する必要があり、委譲後の再使用・同一 capability の
  重複引数・分岐 join（`if c { f(cap) } else { g() }` の後に `cap` を使う）に対するテストを伴う。
  分岐 join の `MergeConflict` 判定自体は既存のままで足りる。
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
**`Unknown`（タイムアウト）は narrowing 拒否とする**（権限判断では警告扱いにできない）。

**regex の近似は権限判断に使ってはならない**。`parse_constraint_to_z3_string()` の `matches` 処理は
`^p.*` / `.*s$` / `.*sub.*` などを prefix / suffix / contains へ**近似**するもので、元の正規表現より
広い集合を認めうる。この近似を `C1 ⟹ C2` の判定に使うと、実際の constraint が拒否するアクセスを
Z3 が許可してしまう（unsound な権限拡大）。したがって最小サブセットでは:

- capability の constraint に `matches(...)` を許さない。許可するのは厳密にエンコードできる
  `starts_with` / `ends_with` / `contains` / `not_contains` とその `&&` 連結のみとする。
- 近似経路に落ちる入力（`matches` や `None` を返す制約）はエラーとして拒否する。既存の effect 検証における
  `matches` の扱い（近似 + 警告）は変更しない。制限は capability の constraint にのみ適用する。

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

ただしこの `is_fn_type()` ゲートは `verify_effect_containment()` だけではなく
`verification/executor.rs:573` と `verification/support/dataflow_inference.rs:431`（パラメータの
`effect_set` からのエフェクト推論）にも存在する。capability パラメータの effect を見落とさないためには
**3 箇所を同じ規則で拡張する必要がある**（共通のヘルパを導入し 3 箇所から呼ぶのが望ましい）。
この拡張は Stage 1 の作業項目であり、いずれも条件の拡大のみで既存の関数型パラメータの扱いは変わらない。

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

この範囲では capability 値はランタイム表現を持たず、constraint は §3 の検証で消費され、
実行時には残らない。**現行の zero runtime overhead は維持できる。**

ただし「`Rvalue::Grant` を codegen で捨てる」だけでは足りない。`mumei-emit-llvm/src/codegen/driver.rs`
の `compile_atom_into_module()` は `atom.params` を 1 対 1 で LLVM のパラメータ型に写像し
（`resolve_param_type()`）、entry で `function.get_nth_param(i)` を束縛する。capability パラメータを
そのまま残せば消去できない値が ABI に現れ、逆に `grant` 側だけを消すと引数に渡す値がなくなる。
Stage 2 には **ABI レベルの消去パス**が必要である:

- 関数定義・宣言のパラメータ列から capability パラメータを取り除き、残りのパラメータの索引を詰め直す。
- 直接呼び出し（`HirExpr::Call`）の実引数列から対応する引数を同じ規則で取り除く。
- `atom_ref` 経由の間接呼び出しを capability パラメータについてはサポート対象外とする
  （capability を関数値の引数型に含めない。§4.3 の制限と同じ理由）。

この消去は「型レベルの抽象を単相化で落とす」既存のエフェクト多相の処理と同種であり、
新しいランタイム機構は不要だが、Stage 2 の作業項目・テスト（消去後の LLVM IR が
capability 導入前と一致すること）として明示的に含める必要がある。

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

| Stage | 内容 | 完了条件 | 状態 |
|---|---|---|---|
| Stage 1 | `capability` 型宣言（コンテキスト依存キーワード）+ capability 型パラメータのみ。`grant` なし。`is_fn_type()` ゲート 3 箇所（`effects.rs` / `executor.rs` / `dataflow_inference.rs`）の拡張を含む | 既存 effect と等価な検証結果になること。`.mm` 回帰ゼロ | ✅ 実装済み（2026-08-30、`docs/ROADMAP.md` P29） |
| Stage 2 | `grant E where C` 式と静的 capability 束縛。codegen は消去のみ（§4.2 の ABI 消去パス: 定義・宣言・直接呼び出しから capability パラメータと実引数を除去）を含む | `grant` を含む新規テストが通り、消去後の LLVM IR が capability 抜き版と一致し、既存 codegen 出力が不変 | 保留（タスク 3 の結論が否定のため着手しない。将来のトリガ観測待ち） |
| Stage 3 | narrowing（`grant cap where C'`）と `C1 ⟹ C2` の Z3 判定 | narrowing の受理 / 拒否が Z3 で判定でき、`Unknown` 時の安全側動作が定義されている | 保留（同上） |
| Stage 4 | move ベースの revocation（アフィン capability）。`Rvalue::Call` の引数に対する所有権移動を新規実装（§2.2） | 委譲後の再使用 / 重複引数 / 分岐 join に対して use-after-move / double-move / `MergeConflict` が報告される | 保留（同上） |

### Stage 1 の実装結果（2026-08-30）

構文は §1.4 案 1（コンテキスト依存キーワード）を採用した:

```mumei
effect SafeFileRead(path: Str) where starts_with(path, "/tmp/");
type FileCap = capability SafeFileRead(path: Str) where starts_with(path, "/tmp/");

atom read_log(cap: FileCap, user_id: Str)
    effects: [SafeFileRead(path)]
    requires: not_contains(user_id, "..") && not_contains(user_id, "/");
    ensures: result >= 0;
    body: { let path = "/tmp/" + user_id + ".log"; perform cap.read(path); 1 }
```

`capability` は `type X = ` の直後だけキーワードとして解釈し、それ以外では `Token::Ident`
のままにした（`grant` はトークン化を一切変更していない）。したがって `capability` /
`grant` を変数名・パラメータ名に使う既存ソースは壊れない。

- capability パラメータは `TypeRef.effect_set = Some(["SafeFileRead"])` と
  `TypeRef.capability = Some(CapabilityType { effect, constraint })` を持つ。
  effect containment / propagation / エフェクト推論の 3 箇所のゲートは `is_fn_type()` から
  共通ヘルパ `TypeRef::carries_effects()`（関数型 **または** capability 型）に置き換えただけで、
  比較式 `param_leaves ⊆ allowed_leaves` は不変。
- `perform cap.op(x)` はパーサが裏側のエフェクト（`perform SafeFileRead.op(x)`）へ解決するため、
  MIR / codegen / proof certificate の経路は capability を意識しない。capability 宣言の constraint は
  同一エフェクトを指す capability パラメータから引き、既存の `check_constant_constraint()` /
  `parse_constraint_to_z3_string()` にエフェクト制約と並べて渡す（新しい制約言語・Z3 sort なし）。
- 失敗はすべて既存分類で報告される（呼び出し元がエフェクトを宣言していない場合は
  effect polymorphism violation、constraint 違反は既存の effect 制約と同じ経路）。
- Stage 1 の制限: capability 宣言はそれを宣言したモジュール内のパラメータにのみ適用される
  （import 越しの capability 型パラメータ、および REPL で capability 宣言と atom を別入力で
  投入した場合は Stage 2 以降で resolver に載せる）。
- Stage 1 の制限: perform サイトから capability レシーバの同一性が失われるため、1 つの atom が
  同一エフェクトに対する複数の capability パラメータを取ると、各 constraint が全 perform に
  連言で適用される（権限は広がらないが正当なプログラムを過剰に棄却しうる）。同じ atom 内の直接
  `perform Effect.op(x)` も capability constraint を継承する。レシーバを構文木に保持した
  per-receiver 解決は Stage 2 で行う。
- Stage 1 の制限: capability constraint の検証範囲は既存 effect 制約と同一である。定数および
  定数から導出される引数は Z3 で検査されるが、`requires` で束縛されていない完全に記号的な引数
  （例: `perform cap.read(user_path)` で `user_path: Str` に制約がない）は既存 effect でも受理される
  ため、capability でも受理される（`origin/develop` の同等な effect 版でも同じ verdict であることを
  CLI で確認済み）。capability という名称に反して権限の実行時強制は行わない。
  capability の消去 ABI パスも Stage 2 のまま（`grant` がないため runtime 表現は生じない）。

非対象（本調査の前提を壊すため、必要になった時点で改めて調査する）:
value-dependent constraint、capability の data structure への格納、
分岐による capability の動的選択、動的 revocation、
capability constraint での `matches(...)`（近似が権限を広げるため、§3.1）、
capability subtyping における effect 階層の利用（親 effect への代入は権限拡大、§2.1）。

---

## 7. 総合結論

**実装フェーズに進んでよい（肯定的）。** 4 調査項目すべてで opt-in 判定基準
「`grant` を使わない既存 `.mm` が現行セマンティクスのまま通る」が充足され、
破壊的変更が不可避である証拠は見つからなかった。決め手は次の 3 点である:

1. capability パラメータの effect 部分は `TypeRef.effect_set` を持つパラメータとして表現でき、
   effect containment / propagation の不等式を**書き換えずに**再利用できる（§3.2）。
   constraint は `effect_set` に載らないため capability 専用の型フィールドを追加するが、
   既存 `.mm` では常に `None` であり型表現の意味は変わらない（§1.1）。
2. capability の constraint は既存の文字列制約断片と同一で、新しい Z3 sort も
   新しい制約言語も不要（§3.1）。
3. `movability_from_type()` が未知の型名を `Move` に分類するため、アフィンな capability は
   MIR move 解析の骨格にそのまま載る。追加実装は呼び出し地点の所有権移動に限られる（§2.2）。

新規実装が必要と判明した箇所は 2 つ（Stage 4 の呼び出し地点 move、Stage 2 の ABI 消去パス）で、
いずれも既存の意味論を書き換えずに追加できるため、判定は肯定のままである。

一方で、Section 3 の "Disadvantages" が挙げる「ランタイム表現が必要」「破壊的変更」は、
最小サブセット（静的 capability・閉じた constraint・データ構造に載せない）に限れば**回避できる**ことが
本調査で確認できた。逆に、この境界を越える機能（§6 非対象）を初回から取り込むと、
zero runtime overhead と Z3 の決定可能断片の両方を失うため、その場合は
Option A（parameterized effects + Z3）継続が正しい判断となる。

なお本調査が答えたのは Priority 15 のタスク 1（非破壊な設計調査）とタスク 2（互換性判定）であり、
本調査の「肯定的」は **技術的に着手可能である**という判定であって、着手すべきという需要判断ではない。

タスク 3（AI エージェント側で capability delegation の需要が実在するかの検証）は
✅ **調査完了・結論は否定**である。`mumei-lang/mumei-agent` の
[`docs/CAPABILITY_DEMAND_STUDY.md`](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/CAPABILITY_DEMAND_STUDY.md)
（PR [mumei-agent#567](https://github.com/mumei-lang/mumei-agent/pull/567)、マージ済み）が、
エージェント側に Stage 2 以降を要求する実需要は観測されないと結論した。したがって
Stage 2〜4 は将来のトリガ観測まで保留し、着手しない。既に実装済みの Stage 1
（`docs/ROADMAP.md` P29）はタスク 3 の結果に依存しない非破壊な範囲であり、撤回せず維持する。

したがって `docs/CAPABILITY_SECURITY.md` §4 の Recommendation は撤回しない。
Option A を既定パスとして継続し、Stage 1 のみが opt-in 拡張として上積みされている、という位置づけとする。
