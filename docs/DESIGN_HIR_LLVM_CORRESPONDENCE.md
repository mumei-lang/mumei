# `hir → llvm_ir` 昇格の調査メモ

`DESIGN_OBJECT_TRANSLATION_CORRESPONDENCE.md` §8 残件(1)の調査。
実装判断は別 PR。このメモは現状の語彙・既存機構・昇格に必要な
構成要素と段階案・決定点を整理する。

## 1. 現状の edge

`SemanticContract.transformations` の6段のうち、emitter lowering に
対応するのは:

- `from: "hir"`, `to: "llvm_ir"`, `relation: "existing_llvm_emitter"`,
  `output_identity: None`,
  `validation: "unchecked_not_translation_validated"`
  (`LLVM_CORRESPONDENCE_STATUS` 定数、
  `semantic_contract.rs:2293`)。
- 同じ定数が `SemanticContract.correspondence` フィールドにも使われる
  （contract 全体の correspondence 記述）。

現状この edge が持つのは HIR/MIR の **fingerprint** のみ
（compiler-local identity。等価性の証拠ではない、と exclusions に
明記済み）。

## 2. 既存機構と、この gap との距離

### 再利用できるもの

- **IR 側エンコーダ**（`mumei-emit-llvm/src/translation_validation.rs`）:
  `parse_ir_module_for_comparison` が emit 済み `.ll` を
  `check_function`（arity/header/body ゲート）で受理し、
  `encode_ir_function_for_comparison` /
  `encode_parsed_function_for_comparison` が比較モデル
  （`model.inputs` 上の Z3 項 + definedness obligations）へ持ち上げる。
  scalar-i64・call・bounded memory・整数集約/sret・i128・f64 leaf の
  サブセットと post-O2 プロファイルまで整備済み（§7 Steps 3a–3f）。
- **比較の型**: `compare_function` は LLVM 関数を内部で encode して
  **source モデル**（`PureI64Body`）と照合する — iki encode 済み
  modeli parametre olarak kabul etmez. Dolayısıyla "aynı model uzayında
  iki tarafı karşılaştır" kalıbı (inputs paylaşımı, covering-edge
  return eşitliği, definedness yön kontrolü) yeniden kullanılır ama
  fonksiyonun kendisi değil: iki encode edilmiş modeli kabul eden yeni
  bir iki-model karşılaştırma girişi gerekir.
- **`recheck-llvm`（C09）は anlamsal karşılaştırma yapar** —
  `validate_native_main`/`validate_pure_i64_module` üzerinden
  kaynak modeli ile IR'i karşılaştırır ve `Outcome::Different` /
  `UndefinedBehavior`'ı `Different` olarak döndürür. Ancak çapası
  **source**'tur, HIR değil.

### 埋まっていないもの

- **HIR→モデルの独立エンコーダが存在しない**。HIR は
  normalization/monomorphize 後（名前 flatten・型展開・HirExpr
  バリアント）なので、source-atom 用エンコーダを HIR に転用するのは
  意味論的に誤り — `hir→llvm_ir` の主張は「lowerer が HIR 意味論を
  保存したか」であり、モデルは HIR から組み立てる必要がある。
- `recheck-llvm`（C09）`tv::validate_native_main` /
  `validate_pure_i64_module` üzerinden **anlamsal** source↔LLVM-IR
  karşılaştırması yapar (`Outcome::Different`/`UndefinedBehavior` →
  `Different`) — ama çapası **source**'tur, HIR değil.
- `translator_ir` メタデータ・`bridge_lemma_hash` も identity 系。

Bu yüzden bugün HIR→IR yanlış bir lowering yakalanamaz: machine↔IR
uyuşması, makinenin (yanlış indirgenmiş olabilen) IR'e sadık olduğu
anlamına gelir; `recheck-llvm`'in source↔IR uyuşması da uyuşmazlığın
`source→HIR` mi yoksa `HIR→IR` mı aşamasında doğduğunu ayıramaz.
Ancak HIR'in kendisi referans alınırsa iki yarı (`source→HIR` ve
`HIR→IR`) ayrı ayrı doğrulanabilir.

## 3. 昇格に必要な構成要素

1. **HIR→model エンコーダ**（新規・本件のコスト本体）:
   `HirSignature`/`HirStmt`/`HirExpr` を `model.inputs` 上の同じ
   SSA モデル語彙（Z3 Int/BV + definedness + bounded-memory 配列）
   に encode。`expr_emit`/`stmt_emit` と同じバリアント網羅が必要。
   IR 側と同じ staged 戦略が妥当（scalar-i64 → call → bounded
   memory → aggregate/sret → i128 → f64 leaf の順、各段 fail-closed
   で `outside_profile`）。
2. **比較**: `compare_function` LLVM fonksiyonunu içeride encode edip
   source modeliyle karşılaştırır — HIR encoder `PureI64Body` üretirse
   aynı giriş doğrudan yeniden kullanılır (ek bir iki-model
   karşılaştırıcı gerekmez). Uygulama bunu doğruladı:
   `encode_hir_pure_i64_leaf` + `validate_hir_pure_i64_leaf` sarmalı
   (#1408).
3. **語彙**: 新しい validation 名（候補
   `checked_hir_llvm_model_correspondence`）— 新語彙なので採用時に
   承認を取る。実行されなかった場合の fail-closed は
   `unchecked_not_translation_validated`。
4. **実行面（要決定）**: どこで検査を走らせるか —
   (a) `native-bundle` の recheck 系 descriptor、
   (b) `recheck-llvm`/`evidence` 側の検査項目追加、
   (c) build 時の opt-in。emitter codegen は共有資産で
   C03 系レーンと衝突し得るため実装レーン割りは親と要相談。
5. **emit 面**: `transformations[hir→llvm_ir].validation` の昇格は
   §8(2) と同じ profile 条件付き emit が自然。
   `SemanticContract.correspondence` フィールドを同時に動かすかは
   別決定（contract の宣言フィールドであり verdict ではないが、
   消費者の読み方を変える点は注意）。

## 4. 段階案（実装時の推奨順）

1. エンコーダ骨格 + scalar-i64 leaf 関数の比較（最小 e2e）
2. call → bounded memory → aggregate/sret → i128 → f64 leaf
   （§7 と同じ拡張順、各段で fail-closed 回帰テスト）
3. profile 条件付き emit + descriptor 反映 + docs
   （着地済み: `emit_native_hir_llvm_transformation` が native-bundle
   prepare 経路のみで `validation` を
   `checked_hir_llvm_model_correspondence` に更新。recheck は
   `hir_llvm_model_correspondence` 検査項目で各 atom の
   `validate_hir_pure_i64_leaf` 結果を集約し、transformation 項目の
   `result` に同値を emit）

工数感: IR エンコーダが Steps 3a–3f で8 PR 掛かった規模。
HIR 側は HirExpr バリアント網羅＋単体テスト群で同等〜それ以上の
スライス数を見込む（別メモ級という当初評価と整合）。

## 5. 決定点（実装着手前に確認）

- 検査の実行面を (a)(b)(c) のどれにするか（レーン衝突・拒否面の
  fail-closed 方針を含む）
- 新語彙名の承認（`checked_hir_llvm_model_correspondence` 案）
- `SemanticContract.correspondence` も動かすか、
  transformations のみに留めるか
- HIR モデルが source モデルと共有する項の定義域合わせ
  （u64/BV、f64/ieee754、definedness の方向規約を IR 側と揃える）
