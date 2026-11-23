# machine-TV LLVM-IR モデル比較（Step 3a）設計メモ

ステータス: 設計提案（実装なし）。`docs/DESIGN_OBJECT_TRANSLATION_CORRESPONDENCE.md`
§7 Step 3(a) の分解メモ — machine-TV の比較対象を「ソースモデル」から
「LLVM IR から再導出したモデル」に切り替え、`llvm_ir → object_code` を
直接の等価検証として主張できる形にする設計を固定する。

## 1. 目的とスコープ

昇格条件 4 の本筋 (a): machine-TV が証明している「機械語 ↔ ソースモデル」の
等価を、「機械語 ↔ LLVM IR モデル」の等価に置き換える。これにより
`llvm_ir → object_code` の変換関係は instruction selection・レジスタ割当を
含む命令列対応として検証され、`checked_object_translation_tv` の
`validation` 語彙が使えるようになる（昇格条件 4 の (b) で導入済みの
`checked_source_machine_tv_within_profile` は source↔machine の等価に
限定した名前で、IR↔object を名乗れない — 本設計がその穴を埋める）。

スコープ外（従来通り deferred / 別チェックのまま）:

- `hir → llvm_ir` 段の昇格（object 段とは別問題。IR モデル比較は
  `llvm_ir` を信頼入力として扱う）
- ループ帰納法・cyclic CFG・MXCSR/NaN・x87 sret・i128 mul limbs・
  `srem i128`（machine-TV の deferred セット。IR モデル化で自動的に
  解消はしない — 除外し続けて fail-closed）
- verdict・grade promotion・producer clock・形式的証明

## 2. 現状の基盤

Step 4（#1363）で `llvm_ir_object_equivalence` 検査項目として直接比較の
骨格は実装済み:

- `translation_validation::encode_ir_function_for_comparison` が
  `program.ll` をパースし、関数を「パス条件付き IR 戻り値の
  `(path_condition, return_value)` edge リスト + 定義域義務
  `(path_condition, defined)`」にエンコード（同一 Z3 context、
  `optimized` フラグで select 畳み込み等を許容）
- `machine_tv::compare_object_function_against_ir` が各 lifted machine
  パスについて、ある IR edge がカバーし（パス条件が整合）、その
  定義域義務が満たされ、戻り値が一致することを検査。未カバー
  パス・発散 edge・定義域義務失敗は `Different`
- `model.inputs` は両側で共有 — 機械語の ABI 読み戻しと IR の仮引数が
  同じシンボリック入力を参照する

現行の制限（scalar-i64 サブセット）:

- 引数・戻り値は単一リーフ i64 のみ（`param_leaf_doubles == [false]`）
- aggregate/f64/wide (i128)/call/loop/bounded memory は `NotLiftable`
- `llvm_ir_object_equivalence` は対象外関数を `unchecked` で列挙 —
  fail-closed だが、IR↔object の昇格条件は全対象関数の checked を要求

## 3. 設計

### 3.1 比較対象の切替点

現行: `evaluate_machine_tv` はソースの semantic model（PureI64Body 等）
から `model.inputs`・定義域義務・期待戻り値を組み立て、lifter の出力と
比較する。Step 3a は比較器の**期待値の出所**を LLVM IR エンコーダに
替える。すなわち:

1. 署名から `model.inputs`（64-bit シンボリック入力列）を導出 — 現行と
   同じ ABI 解釈を両側に適用するため署名の導出元はソースモデルのまま
   維持し、IR 側 arity との整合は既存の arity ゲート（`param_shapes` /
   `array_params` マスク経由）を再利用する
2. IR エンコーダが `(path_condition, return_value)` edges と
   `(path_condition, defined)` 定義域義務を出す
3. lifter の各 machine パスを「全 IR edge の disjunction 内で戻り値一致 +
   定義域義務成立」で検証 — `compare_object_function_against_ir` の
   現行意味論をそのまま昇格させる

比較結果は関数ごとに `Equivalent` / `Different` / `Unknown` /
`OutsideProfile(reason)` の4値で、既存の `MachineTvEvaluation` 流路に
載せる — 新規の outcome 語彙は増やさない。

### 3.2 サブセットの拡張順序

現行 `llvm_ir_object_equivalence` 項目の `NotLiftable` 群を、機械語側
リフトが既に対応済みの順に IR エンコーダへ移植する:

| 順序 | 領域 | 機械語側の既存リフト | IR 側に必要なエンコード |
|---|---|---|---|
| 1 | forward/acyclic branch | 分岐パス列挙済み | `br`/`switch`/`phi` のパス条件列挙（既に scalar-i64 で実施） |
| 2 | call（同一モジュール内直接呼出） | `call_sites` で結果値代入済み | callee を IR エンコードし戻り値 edge を連結（入口 ABI で inputs 写像） |
| 3 | bounded memory（配列引数） | `array_params` + load/store リフト済み | `getelementptr`/`load`/`store` を配列入力上の選択関数でモデル化 |
| 4 | aggregate 引数・戻り値（sret 含む） | leaf 展開・sret バッファ経路済み | struct 引数を leaf 単位で展開、sret は暗黙 `rdi` バッファとして model.inputs に追加 |
| 5 | i128 引数・戻り値 | wide leaf pair・ti3 ヘルパー・shld/shrd 済み | 2 リーフ入力として (lo, hi) ペア化、`mul` limbs は deferred のまま除外 |
| 6 | f64 leaf | f64 leaf リフト済み | SSE `movsd`/`addsd` 等と `fadd`/`fcmp` の対応 — NaN payload・MXCSR は deferred で definedness から除外 |

各段で未対応の関数は引き続き `OutsideProfile`/`unchecked` で fail-closed。
**順序は合否ではなく記録粒度** — 昇格ゲートは「プロファイル対象関数の
全件 checked」を要求するため、サブセット拡張が進むほど昇格可能な
バンドルの範囲が広がる。

### 3.3 出力 identity と記録位置

`llvm_ir_object_equivalence` 項目は既に `object_translation.correspondence`
に `checked`/`unchecked`/`inconsistent` を出している。Step 3a では同じ
項目を IR モデル比較の実装に繋ぎ替え、関数ごとの内訳を既存の
`functions.<name>` 内訳行（`outcome`/`reason`/`detail`）で列挙する。
`unchecked` のまま残る関数は残域として `unchecked_domains` に
`llvm_ir_object_equivalence:<function>` で名前付き列挙 — 沈黙させない。

### 3.4 昇格語彙の接続

- `transformations[llvm_ir→object_code].validation`:
  `not_executed_by_extractor` → 全対象関数 Equivalent + 残域列挙済みで
  `checked_object_translation_tv`（4(a) 専用の名前）
- 部分昇格なし・必須条件の未実施は inconclusive/not_performed で
  fail-closed（昇格条件 7 と同じ規則）
- `checked_source_machine_tv_within_profile` との関係: IR モデル比較が
  source↔machine ゲートを**置き換えるわけではない**。`hir→llvm_ir` 段が
  未検証のまま残るため、source↔machine の `checked_*` は source モデル
  側の等価として独立に残し、IR↔object の `checked_object_translation_tv`
  は IR を信頼入力とする等価として別に立てる。両者を単一の
  「コンパイル全体等価」に畳み込まない（`hir→llvm_ir` の昇格が来るまで
  接続しない — 昇格条件の語彙混同を防ぐ）

## 4. 最小再現・計測ケース

- `atom add(a,b) -> i64`（scalar-i64、現行 subset）: IR edge が `add` の
  み → Equivalent、unchecked 0 件で `checked_object_translation_tv` の
  最小形
- `atom max(a,b)`（`icmp`+`select` / `cmov`）: select 畳み込み edge の
  パス条件が `cmov` パスと整合することを検証（現行でも通る回帰ケース）
- `atom twice(x) { inner(x)+inner(x) }`（call 導入後）: callee の IR edge
  が caller の inputs 写像で連結されることを検証
- 負例: IR の `udiv`（定義域義務 `b != 0`）を含む関数で `b=0` が機械語の
  definedness に覆われない場合 `Different` → fail-closed を確認
- 境界: `mul i128`・cyclic 関数を含むバンドルで当該関数のみ
  `unchecked` 列挙され、他関数の checked と両立することを確認

## 5. 未解決の設計問い

- 複数 edge の disjunction が増えた場合の SMT コスト（関数あたりの edge
  上限を設けるか、パス条件の事前正規化で絞るか）は計測後に決める
- `optimized` IR（O1+）で phi→select 等の畳み込みが更に進むケースの
  edge 対応をどこまで許容するか（現行の `optimized` フラグの拡張範囲）
- callee の IR エンコードを関数単位でキャッシュするか（同一 callee の
  多重呼出で再エンコードしない）
