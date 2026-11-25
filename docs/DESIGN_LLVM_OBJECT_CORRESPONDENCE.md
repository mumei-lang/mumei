# `llvm_ir → object_code` 昇格設計メモ

ステータス: Step 1（skipped 分解 + `checks_not_performed` 非 `.text` 明示、#1421）、
Step 2（辺 `result`/`validation` 配線、#1423）、Step 3（`object_data_correspondence`
項目、#1429）、Step 3'（`object_symbol_attribute_correspondence` 項目、本差分）
実装済み。Step 4（subset 拡張）は mixed register pair のみ着手（#789）。
`docs/DESIGN_OBJECT_TRANSLATION_CORRESPONDENCE.md`
の §4 条件 4(a) — 「machine-TV の比較対象を LLVM IR 再導出モデルに切り替える」
本筋路線 — が Step 3a–3f スライス（#1382–#1398）として既に着地済みのため、
本メモは残りの昇格条件と語彙・欠落領域を棚卸しする。
`hir → llvm_ir` 側の昇格は `docs/DESIGN_HIR_LLVM_CORRESPONDENCE.md` と
#1415 で完走済み。Step 2（#1423）で native-bundle の transformations から
`unchecked_not_translation_validated` のまま残る辺はなくなる
（残域は `result`/`detail`/`checks_not_performed` が引き続き報告）。

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
   → **Step 3' として実施済み**: `object_symbol_attribute_correspondence`
   項目が IR linkage ↔ object シンボル binding（global/local/weak）を
   照合する。calling convention は ELF シンボルに表現されないため
   binding 比較が正直な部分集合。`unnamed_addr` 実体は除外（シンボルが
   統合・省略され得て binding 主張が残らない — バイトは
   `object_data_correspondence` が担保）。導出不能 linkage
   （`available_externally`/`appending`/DLL 系等）は `underivable` で
   fail-closed。`.text` 辺昇格ゲートには含めない（Step 3 と同じ位置づけ）。
5. **IR 由来の非ソース関数** — C `main` ラッパー等、ソース atom に
   対応しない IR メンバーは `llvm_ir_object_equivalence` の対象外
   （`machine_code_entry_wrapper_tv` が別 anchor で担当 — 辺の集約には
   含めない設計が自然）。
6. **IR encode の残域** — `encode_ir_function_for_comparison` が
   `skipped` に落とす形状（非スカラー call site、集約 f64、複合引数、
   unsupported instruction を含む関数）が detail に理由別で残らない。
7. **`skipped` が2つの異なる意味を持つ** — `NotLiftable` は
   「subset 外で比較を試行していない」（`unsupported_signature` 等）と
   「比較を実行したがソルバーが判定不能」（`solver_unknown`、
   machine_tv.rs の `SatResult::Unknown` 分岐）を同じ `skipped` に
   畳む。後者は比較試行済みの未判定で、fail-closed では
   `checked` に数えてはいけない区別が必要。

## 4. 昇格条件

必須条件を全て満たした場合のみ昇格（部分昇格なし、判定不能は
`unchecked` で fail-closed）:

1. **関数列挙の完全性**: `object_symbol_correspondence` checked
   （再利用 — IR 関数→object シンボルの全対応）。
2. **呼び出し・リンク対応**: `object_call_edge_correspondence` と
   `object_link_correspondence` が checked（再利用 — 従来の昇格条件を
   維持。recheck には link 照合が走らない経路があるため、項目の
   存在そのものも要求する — 未実施は unchecked で fail-closed）。
3. **命令列対応**: `llvm_ir_object_equivalence` が
   `inconsistent` でないこと — `different` が1関数でもあれば辺は
   `inconsistent_correspondence`（#1415 で導入した語彙を踏襲）。
   in-subset 関数が1つ以上 equivalent で残りが skipped のみの場合の
   扱いは §7(b) の決定に従う。
4. **relocation 記録**: `relocation_targets`・`relocation_bounds`・
   `relocation_resolved_values`/`relocation_applied_values` が
   checked（再利用 — Step 4 着地済み）。
5. **非決定性の排除**: 正規化 argv 比較 + digest pin が一致
   （`canonical_argv_compared`、再利用）。
6. **残域の列挙**: skipped 関数（理由別カウント + solver_unknown の
   区別、§3-7）、非 `.text` セクション、deferred セット相当域が
   `detail` / `checks_not_performed` に残ること。
7. **fail-closed**: 上記いずれか判定不能 → `unchecked_not_translation_validated`。

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

- **Step 1（残域の正直な列挙・昇格の前提）**: `skipped` を
  「subset 外（`unsupported_signature`/`unsupported_ir`/
  `unsupported_instruction`）」と「比較実行済みだが未判定
  （`solver_unknown`）」に分解し、後者は `unknown` として detail に
  残す（checked には数えない — §3-7）。合わせて
  `checks_not_performed` に非 `.text` セクション
  （`.rodata`/`.data`/`.bss` 未比較）を明示。**この Step が
  §4-6 の残域列挙を満たすまで Step 2 の昇格配線は行わない** —
  現状の集約のまま辺を checked にすると solver_unknown が
  紛れて fail-closed 原則に反する。
- **Step 2（辺の配線・最小）**: descriptor の `llvm_ir→object_code` 辺に
  `result` を追加し `llvm_ir_object_equivalence` 項目の集約を反映
  （contract 側の `validation` は emit 済みなら profile 条件で更新）。
  in-subset 全関数 equivalent・solver_unknown なしの bundle が
  `checked_correspondence` になる最小差分。テスト: 既存
  `transformations[0]` アサートに `result` 追加＋divergent ケース
  （unit-level、source identity ゲートで CLI からは作れない —
  #1415 と同じ制約）。
- **Step 3（データセクション照合・別スライス推奨）**: `.rodata` 定数 ↔
  IR グローバル初期値の対応項目 `object_data_correspondence`
  （`.text` 辺の昇格ゲートには含めない — 別 item として追加）。
  実装メモ: IR 側は inkwell でグローバル初期値を byte 導出
  （`ir_data_globals` — i64 以下の整数・f32/f64・i8 文字列・
  一様要素配列が導出可能、pointer/struct/超 64bit は `underivable`
  で fail-closed）。object 側は非 `.text` の defined シンボル
  （local 含む — `str_lit` は private→local）＋初期化データ
  セクションの生 bytes を保持。名前一致で byte 比較し、
  名前不在は `unnamed_addr` 定数マージ（anonymous 定数は
  `.rodata.str1.1` にシンボル無しで置かれる）を考慮してセクション
  生 bytes の内容一致で `merged` と記録、見つからない場合は
  `missing`（`unchecked` — `different` ではない）。`.bss` は
  size の zero image と照合。全 derivable global が検証されれば
  `checked`、byte 乖離で `inconsistent`。
- **Step 3'（シンボル属性照合・本差分）**: 欠落4 の実施 —
  `object_symbol_attribute_correspondence` 項目を `correspondence.checks`
  に追加。IR 側は `ir_symbol_attributes` が defined な named entity
  （関数 + グローバル）の linkage を期待 binding（global/local/weak）に
  写像し、object 側は defined な named シンボル全件（text・data、
  local 含む）の binding を抽出して名前一致で比較。`unnamed_addr`
  実体は対象外、導出不能 linkage・同名多重定義・シンボル不在は
  `underivable`/`conflicting`/`missing` として `unchecked` に倒す。
  `.text` 辺の昇格ゲートは変更しない（Step 3 と同じく別 item）。
- **Step 4（#789、部分解凍）**: Linux x86-64 の flat な LLVM 戻り値
  `{ i64, double }` / `{ double, i64 }` のみ追加する。既存の INTEGER/SSE
  リフトを再利用し、`rax` と `xmm0` を LLVM のフィールド順で連結して
  ビット単位で照合する。比較は対象の LLVM IR と object の関数本体の間に限り、
  optimizer の履歴・linker・runtime・実行ファイルの正しさは主張しない。
  nested mixed 結果、mixed `sret`、集約引数、非 scalar-i64 call は今回対象外。
  x87 sret はリフト未対応、MXCSR/NaN の演算意味論はモデル外、i128 mul
  limbs と `srem i128` recheck はそれぞれ異なる比較辺でソルバ未決着のため
  凍結を継続する。cyclic CFG / loop beyond bound も、既存の有限展開が
  ループ帰納法を代用しないため凍結を継続する。#1446 の Lean/Sail 命令照合は
  別段階であり、受理条件・verdict・grade は変更しない。

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
  列挙（沈黙させない）」で統一。ただし2条件付き:
  (i) skipped は subset 外のみに限定し、`solver_unknown`
  （比較実行済みの未判定）は `unknown` として別扱い —
  1件でもあれば辺は `unchecked`（fail-closed、§3-7）;
  (ii) Step 1 の理由別分解が先 — 現状の一律 skipped のまま
  `checked` を名乗ると subset 外と未判定の混入が読み取りにくい。
- **(c) 非 `.text` 対象外の宣言**: `.rodata`/`.data` をこの辺の
  昇格ゲートに含めるか — **推奨**: 含めない。`llvm_ir→object_code`
  は命令選択・レジスタ割当の relation 名
  （`llc_instruction_selection_register_allocation`）のままなので、
  データ配置は Step 3 の別項目で扱う方が relation 名と整合。
  ただし `checks_not_performed` に「data sections unverified」を
  明示列挙する（§4-5）。→ Step 3 で `object_data_correspondence`
  項目が `correspondence.checks` に入ったため、明示列挙の
  `object_data_section_correspondence` エントリは除去済み。
- **(d) C `main` ラッパーの扱い**: `llvm_ir_object_equivalence` の
  対象外（ソース atom に対応しない IR メンバー）。**推奨**: 現状維持 —
  `machine_code_entry_wrapper_tv` が独立に担保しており、辺の集約に
  混ぜると anchor が曖昧になる。
