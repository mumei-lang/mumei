# `llvm_ir → object_code` 昇格設計メモ

ステータス: 設計提案（実装なし）。`docs/DESIGN_OBJECT_TRANSLATION_CORRESPONDENCE.md`
の §4 条件 4(a) — 「machine-TV の比較対象を LLVM IR 再導出モデルに切り替える」
本筋路線 — が Step 3a–3f スライス（#1382–#1398）として既に着地済みのため、
本メモは残りの昇格条件と語彙・欠落領域を棚卸しする。
`hir → llvm_ir` 側の昇格は `docs/DESIGN_HIR_LLVM_CORRESPONDENCE.md` と
#1415 で完走済み。native-bundle の transformations で
`unchecked_not_translation_validated` のまま残るのはこの辺のみ。

## 1. 目的とスコープ

`llvm_ir → object_code` の transformation エントリを
`unchecked_not_translation_validated` から昇格させるために、
producer が何を検証済みか・何が欠けているか・昇格の段階案と語彙を列挙する。

スコープ外（従来通り fail-closed / 別チェックのまま）:

- ABI・runtime・OS 観測、optimizer 一般の意味論、完全な object TV
- deferred セット: x87 sret、MXCSR/NaN、i128 mul limbs、srem i128 recheck-llvm、
  loop induction beyond bound、cyclic CFG
- verdict・grade・checks リストの昇格語彙（従来の制約どおり対象外）

## 2. 現状の棚卸し — 再利用可能な機構

| 機構 | 内容 | IR↔object 昇格への位置づけ |
|---|---|---|
| `llvm_ir_object_equivalence` 検査項目 | ソースモデルごとに `encode_ir_function_for_comparison` で IR 辺（path condition・return edge・definedness・call 引数 obligation）を作り、`check_function_against_ir` が機械語パスと照合。関数ごと `equivalent`/`different`/`skipped` で集約済み | **本筋の照合機構・既に emit 済み**（recorded only — verdict 昇格は deferred 注記つき） |
| `check_function_with_frame_memory_against_ir` | `evaluate_machine_tv` が lifted 関数で IR 辺を expected value に使う経路（Step 3a–3f） | machine-TV 本体でも IR↔機械語等価を実施済み |
| `ir_object_signature_liftable` ゲート | 64-bit スカラー署名（整数＋f64 パススルー/select）、i128/u128 ワイドスカラー（rdx:rax 1辺）、全 i64 集約結果（register/sret memory-class） | 昇格可能な署名範囲の定義そのもの |
| `object_symbol_correspondence` | 全 LLVM IR 関数が object シンボルに対応 | 昇格条件の関数列挙完全性（再利用可） |
| `object_call_edge_correspondence` | 直接呼び出し edge の照合 | 同上（再利用可） |
| `object_link_correspondence` + `relocation_*` 項目 | object→exe のバイト・call 解決一致、全セクション reloc 記録＋`applied_offset` 適用値照合（Step 4） | link 側は checked 済み — 本辺の対象外だが昇格ゲートの一部 |
| `toolchain_object_executable_reproduction` | 正規化 argv ハッシュ + digest pin | 非決定性排除の条件（再利用可） |

## 3. 欠けている部分

1. **辺の `result`/`validation` が項目を消費していない** — 最大の穴。
   `llvm_ir_object_equivalence` が `checked` を返しても
   `llvm_ir→object_code` transformation は `result` キーすら持たず
   `unchecked_not_translation_validated` のまま（`object_code→linked_executable`
   や `hir→llvm_ir` は既に `result` を持つ — この辺だけ不整合）。
2. **skipped 関数の扱いが集約に埋もれる** — `NotLiftable` は `skipped` として
   detail に並ぶだけ。out-of-subset 関数を含む bundle で辺を
   `checked_*` と呼ぶかどうかの規則が未定義（hir 辺は outside-subset を
   detail に列挙した上で checked を許容する実装 — 同じ規則を使うか要決定）。
3. **非 `.text` 内容は一切比較されない** — `.rodata` 定数（文字列リテラル
   グローバル）、`.data`/`.bss` 初期イメージ、関数間パディング、
   セクション属性。`llvm_ir_object_equivalence` は `.text` シンボルの
   命令列のみが対象。
4. **シンボル属性の忠実性** — `object_symbol_correspondence` は名前と
   定義の存在を見るだけで visibility/linkage/calling convention は未照合。
5. **IR 由来の非ソース関数** — C `main` ラッパー等、ソース atom に
   対応しない IR メンバーは `llvm_ir_object_equivalence` の対象外
   （`machine_code_entry_wrapper_tv` が別 anchor で担当 — 辺の集約には
   含めない設計が自然）。
6. **IR encode の残域** — `encode_ir_function_for_comparison` が
   `skipped` に落とす形状（非スカラー call site、集約 f64、複合引数、
   unsupported instruction を含む関数）が detail に理由別で残らない。

## 4. 昇格条件

必須条件を全て満たした場合のみ昇格（部分昇格なし、判定不能は
`unchecked` で fail-closed）:

1. **関数列挙の完全性**: `object_symbol_correspondence` checked
   （再利用 — IR 関数→object シンボルの全対応）。
2. **命令列対応**: `llvm_ir_object_equivalence` が
   `inconsistent` でないこと — `different` が1関数でもあれば辺は
   `inconsistent_correspondence`（#1415 で導入した語彙を踏襲）。
   in-subset 関数が1つ以上 equivalent で残りが skipped のみの場合の
   扱いは §7(b) の決定に従う。
3. **relocation 記録**: `relocation_targets`・`relocation_bounds`・
   `relocation_resolved_values`/`relocation_applied_values` が
   checked（再利用 — Step 4 着地済み）。
4. **非決定性の排除**: 正規化 argv 比較 + digest pin が一致
   （`canonical_argv_compared`、再利用）。
5. **残域の列挙**: skipped 関数（理由別カウント）、非 `.text` セクション、
   deferred セット相当域が `detail` / `checks_not_performed` に残ること。
6. **fail-closed**: 上記いずれか判定不能 → `unchecked_not_translation_validated`。

## 5. 昇格後の語彙

- `transformation.validation`（手法名、値）:
  `checked_llvm_ir_object_model_correspondence` — IR 再導出モデルとの
  有界等価を名付ける。hir 辺の `checked_hir_llvm_model_correspondence` と
  対称。
- `transformation.result`（結果）: 項目集約を踏襲 —
  `checked_correspondence` / `inconsistent_correspondence` /
  `unchecked_not_translation_validated`。
- contract 側 emit: `emit_native_object_transformations` と同じ
  profile 条件付き emit 経路で `llvm_ir→object_code` 辺の
  `validation` を更新（§8(2) で確立したパターン — 通常 `verify` は
  6段+object 段 emit なしのまま不変）。

## 6. 段階案

- **Step 1（辺の配線・最小）**: descriptor の `llvm_ir→object_code` 辺に
  `result` を追加し `llvm_ir_object_equivalence` 項目の集約を反映
  （contract 側の `validation` は emit 済みなら profile 条件で更新）。
  現状で in-subset 全関数 equivalent の bundle が `checked_correspondence`
  になる最小差分。テスト: 既存 `transformations[0]` アサートに
  `result` 追加＋divergent ケース（unit-level、source identity ゲートで
  CLI からは作れない — #1415 と同じ制約）。
- **Step 2（残域の正直な列挙）**: `skipped` を理由別カウント
  （unsupported_signature / unsupported_ir / unsupported_instruction）
  として detail に分解 + `checks_not_performed` に非 `.text` セクション
  （`.rodata`/`.data`/`.bss` 未比較）を明示。
- **Step 3（データセクション照合・別スライス推奨）**: `.rodata` 定数 ↔
  IR グローバル初期値の対応項目 `object_data_correspondence`
  （`.text` 辺の昇格ゲートには含めない — 別 item として追加）。
- **Step 4（subset 拡張・deferred 依存）**: cyclic CFG / loop beyond bound 等
  deferred セットの扱いが決まり次第、liftable 範囲を追従拡大。

## 7. 決定点（推奨対応つき）

- **(a) 集約粒度**: 項目単位の集約（`llvm_ir_object_equivalence`）を
  辺に写すか、関数ごと transformation を増やすか — **推奨**: 項目集約を
  そのまま写す。hir 辺（`hir_llvm_model_correspondence` →
  `checked_correspondence`）と同一規則で一貫、関数ごと辺は
  transformations の基数を不定にするため不採用。
- **(b) skipped 関数を含む bundle の辺の値**: 全ソース atom が
  liftable な場合のみ `checked` にするか、outside-subset を
  detail 列挙で残して `checked` を許すか — **推奨**: hir 辺と同じ
  「compared ≥1 かつ diverged 0 で checked、skipped は detail に
  列挙（沈黙させない）」で統一。ただし hir 側と違い skipped には
  「lift できなかった」意味が混ざるため、Step 2 で理由別分解を
  先に実装してから昇格するのが安全（現状の一律 skipped のまま
  `checked` を名乗ると範囲外混入が読み取りにくい）。
- **(c) 非 `.text` 対象外の宣言**: `.rodata`/`.data` をこの辺の
  昇格ゲートに含めるか — **推奨**: 含めない。`llvm_ir→object_code`
  は命令選択・レジスタ割当の relation 名
  （`llc_instruction_selection_register_allocation`）のままなので、
  データ配置は Step 3 の別項目で扱う方が relation 名と整合。
  ただし `checks_not_performed` に「data sections unverified」を
  明示列挙する（§4-5）。
- **(d) C `main` ラッパーの扱い**: `llvm_ir_object_equivalence` の
  対象外（ソース atom に対応しない IR メンバー）。**推奨**: 現状維持 —
  `machine_code_entry_wrapper_tv` が独立に担保しており、辺の集約に
  混ぜると anchor が曖昧になる。
