# `object_translation_correspondence` 設計メモ

ステータス: 設計提案（実装なし）。C10 Option B の残り深層項目。
この文書は `docs/DESIGN_OBJECT_TV_CONSUMPTION.md`（mumei-agent 側、#1044）の
対になる Core 側 producer 設計で、#1324 で導入された
`SemanticContract.transformations` の object 段拡張を検討する。

## 1. 目的とスコープ

`object_translation_correspondence` は「LLVM IR → object code → linked
executable」の変換関係を検証可能な形で記述するチェック語彙として予約済み
（未 performer）。本メモでは、Core の object 段 producer が何を emit し、
`unchecked_not_translation_validated` から昇格するために何を検証すればよいかを
列挙する。

スコープ外（従来通り fail-closed / 別チェックのまま）:

- ABI・runtime・OS 観測（`abi_runtime_semantics`、`os_*_observation`、
  `runtime_observation_semantics`）
- optimizer 一般の意味論（`optimizer_semantics`）
- 完全な object TV（instruction selection の全網羅）— 本提案は
  有界プロファイル内の対応関係 + 列挙可能性に留める

## 2. 現状の棚卸し

object 段で既に実施されるチェック（`native-bundle recheck-*` / `accept-object`）:

| チェック | 内容 | 対象 |
|---|---|---|
| `object_symbol_correspondence` | object ファイルのシンボル表面が LLVM IR の関数表面と一致 | object |
| `object_call_edge_correspondence` | 直接呼び出し edge が照合可能 | object |
| `object_link_correspondence` | linked executable の関数バイト・call 解決が object と一致 | object→exe |
| `machine_code_*_tv` 群 | 機械語パスとソースモデルの等価性（有界プロファイル） | 関数ごと |
| `object_source_machine_code_correspondence` | `accept-object` が emit する受理記述子 | バンドル全体 |
| `machine_code_entry_wrapper_tv` | C `main` ラッパーの 1 回無条件呼び出し | entry |

既存チェックが**やらない**こと:

1. LLVM IR 関数と object 関数の命令列レベルの対応（`hir → llvm_ir` の
   `unchecked_not_translation_validated` と同種の穴が `llvm_ir → object` にもある）
2. relocation の完全な意味照合（call edge と配置は見るが、reloc 解決値の
   全件一致までは主張していない）
3. object 段の変換関係の `SemanticContract.transformations` への記述
   （現状の 6 段は `hir → llvm_ir` で終わる）

## 3. Producer が object 段で emit すべきもの

### 3.1 変換関係（transformations への追加候補）

```
TransformationRelation {
    from: "llvm_ir",
    to: "object_code",
    relation: "llc_instruction_selection_register_allocation",
    output_identity: Some(object_sha256),
    validation: "<昇格後の状態名>",
}
TransformationRelation {
    from: "object_code",
    to: "linked_executable",
    relation: "static_link_relocation_placement",
    output_identity: Some(executable_sha256),
    validation: "checked_object_link_correspondence",
}
TransformationRelation {
    from: "source_atom",
    to: "machine_code",
    relation: "machine_code_translation_validation",
    output_identity: Some(object_sha256),
    validation: "checked_source_machine_tv_within_profile",
}
```

注意: Step 2 で emit する `checked_source_machine_tv_within_profile` は
source↔machine の等価性を主張するもので、乗せるべき変換関係は
`llvm_ir→object_code` ではなく `source_atom→machine_code`。
`llvm_ir→object_code` の `validation` は (a) の直接検証が実装されるまで
`unchecked_not_translation_validated` のまま残す（source↔machine の結果で
埋めない）。

注意: `SemanticContract` 本体の変更は a672 の C03 レーンに属するため、
フィールド追加・`relation`/`validation` 語彙の確定は親セッション経由で
調整する（本メモは提案のみ）。

### 3.2 検証単位と入出力 identity

- **検証単位**: LLVM IR 関数 ↔ object `.text` シンボル ↔ linked executable の
  同名シンボルの 3 点対応を関数ごとに記録する。
- **入出力 identity**:
  - 入力: 固定済み `llvm_ir_sha256`（record ピン）＋ 固定 llc/ld argv
    （`toolchain_object_executable_reproduction` が保証）
  - 出力: `object_sha256`、`executable_sha256`（record `files` エントリ）。
    固定 llc/ld argv の照合自体が `checks_performed` に載っていることも確認する
    — 正規化済み引数ハッシュを持たない古い record では artifact 再検証が
    引数未比較のまま成功し得るため、昇格条件には「比較を実施した」ことも含める
  - 対応表: 関数名 → {IR 関数名、object シンボル、セクション、オフセット、
    バイト範囲、exe 内アドレス} — この対応表自体を descriptor に含めると
    consumer（mumei-agent `native-object-recheck`）が部分検証できる

### 3.3 relocation 記録

`object_link_corresponds` が比較するのは関数バイトと call 解決先のみ。
producer は object の relocation エントリを列挙し、各 reloc の
{適用位置（対象セクション識別子＋セクション内オフセット）、種別（PC32/PLT32 等）、
ターゲットシンボル、addend、適用後の exe 内値} を emit 対象とする。
セクション識別子がないと別セクションの同一オフセット・種別・ターゲットの
reloc を区別できない（既存のリンク照合もセクション単位で読んでいる）。
適用位置と種別がないと同一シンボル・addend の複数 reloc を区別できない
（既存のリンク照合もこの2項目を読み PC 相対値を検算している）。
外部シンボル（libc/runtime stub）は「前提」として区別し、
`machine_code_callee_saved_preservation_assumed_for_external_callees` と
同じ assumption カテゴリに入れる。

## 4. `unchecked_not_translation_validated` からの昇格条件

昇格は「必須条件（1-4, 6, 7）を全て満たし、残域が `checks_not_performed`
として列挙される」場合のみ許可（部分昇格なし）。必須条件が判定不能な
場合の昇格は認めない — 列挙で許容されるのは主張の対象外として明示する
残域（5）のみで、必須チェック自体の未実施は昇格を抑止する:

1. **関数列挙の完全性**: LLVM IR の全関数が object シンボルに対応
   （`object_symbol_correspondence` で実施済み — 再利用可）
2. **call edge 完全性**: 全直接呼び出しが解決・照合済み
   （`object_call_edge_correspondence` で実施済み）
3. **link 対応**: object→exe の関数バイト・call 解決一致
   （`object_link_correspondence` で実施済み — checkable でない場合は
   inconclusive で fail-closed）
4. **命令列対応（本筋の穴）**: `llvm_ir → object` の命令選択・レジスタ割当が
   検証対象。machine-TV は「機械語 ↔ ソースモデル」の等価を既に有界
   プロファイル内で証明しているが、IR↔object の直接対応ではない。
   昇格には次のいずれかが必要:
   - (a) machine-TV の対象を「ソースモデル」ではなく「LLVM IR から再導出した
     モデル」に切り替え、IR↔object 等価として証明する（既存 lift を再利用、
     ソース段の変換検証とは別の責務として切り出し）
   - (b) プロファイル内の全関数で `machine_code_*_tv` が equivalent、かつ
     out-of-profile の関数がゼロ、かつ `functions` セクションに
     OutsideProfile が残らないことを昇格ゲートとする。
     注意: これは source↔machine の等価であり IR↔object の等価ではない
     — machine-TV の比較対象はソースモデルで、IR と機械語の対応はこの段でも
     未検証のまま残る。昇格後の `validation` 語彙も source↔machine の
     範囲に限定した名前にする必要がある
   - (a) が本筋、(b) は source↔machine の中間段階としてのみ許容
     （IR↔object を名乗ってはいけない）
5. **未モデル化残域の列挙**: `unsupported_instruction`・`call_arguments`・
   `frame_memory_bounds`・over-bound ループ・MXCSR/NaN・外部呼び出し前提が
   descriptor の `checks_not_performed` / `functions.<name>.outcome` に
   全件残っていること（昇格対象外の残域を沈黙させない）
6. **非決定性の排除**: 同一入力から同一 digest が出ること（toolchain pin +
   `toolchain_object_executable_reproduction` で担保済み。再ビルドで
   digest が変わった場合は昇格失敗）
7. **fail-closed**: 上記いずれかが判定不能なら inconclusive/not_performed で
   終了し、昇格した validation 値を書かない

昇格後の `validation` 語彙候補: `checked_source_machine_tv_within_profile`
（(b) — source↔machine の等価として正直に名付ける）または
`checked_object_translation_tv`（(a) の直接対応のみに使う）。既存の
`compiler_local_fingerprint_not_equivalence` と `not_executed_by_extractor`
の中間に位置する第3の honesty レベルとして導入を検討。

## 5. `SemanticContract.transformations` との接続点

- 現状: 6 段（source_text→specification、specification→z3_obligation、
  specification→lean_theorem_goal、source_atom→hir、hir→mir、
  hir→llvm_ir=`unchecked_not_translation_validated`）
- 拡張案: `llvm_ir → object_code` と `object_code → linked_executable` の
  2 段を追加。`output_identity` は object/exe の sha256（record pin と一致）
- object 段の relation は native-bundle コンテキストでのみ意味を持つため、
  `extract_pure_i64_contract` のような profile 分岐か、transformations を
  profile 条件付きで emit する設計が必要（単一ファイル検証では object が
  存在しない）
- **連絡事項**: `SemanticContract` 本体変更は a672 の C03 #787 レーンに
  属するため、実装着手前に親セッションへ確認する

## 6. 最小再現・計測ケース

実機プローブ（既に動作確認済みのパターン）:

```bash
# bounded ループ関数（countdown を含む）
mumei native-bundle build loop.mm -o bundle
build_id=<出力された Build ID>
mumei native-bundle recheck-machine-tv bundle loop.mm --expected-build-id "$build_id" --json
# → functions.countdown.outcome = "equivalent_within_profile"
#   checks_performed に "machine_code_bounded_loop_function_tv"
#   checks_not_performed に "machine_code_tv_loop_executions_beyond_bound"
```

昇格判定の計測に使える最小ケース:

1. **全等価ケース**: 全関数が `equivalent_within_profile` の bundle
   （単純な i64 atom 2-3 個 + main）— 昇格条件 1-4 が全て満たされるかを確認
2. **out-of-profile 混入ケース**: 未対応命令（`unsupported_instruction` で
   fail-closed になる形）を含む atom を混ぜ、`functions.<name>.outcome` に
   OutsideProfile が残り昇格が抑止されることを確認。over-bound ループは
   使わない — 上限内の入力では関数が等価と判定されるため抑止ケースに
   ならない（上限外の実行は未検証として `checks_not_performed` に残るが
   関数自体は equivalent_within_profile になり得る）
3. **relocation 破壊ケース**: object の call target を1バイト書き換えた
   bundle では artifact 再現チェックがリンク照合より先に失敗するため、
   CLI 経路ではなく `object_link_corresponds` を直接呼ぶ単体テストで
   `Different` → 昇格失敗を確認
4. **非決定性ケース**: 同一ソースを2回 build して object_sha256 が一致するか
   （不一致なら条件6で昇格失敗）

## 7. 段階的ロードマップ

- **Step 1（メタデータのみ）**: transformations に `llvm_ir→object_code`・
  `object_code→linked_executable` を `validation: "not_executed_by_extractor"`
  で追加し、output_identity に record pin の digest を入れる。
  検証は増えないが変換関係が消えない
- **Step 2（source↔machine の中間段階）**: 昇格条件 4(b) のゲートを producer に
  実装 — 全関数 equivalent + 残域列挙の確認で
  `checked_source_machine_tv_within_profile` を emit（IR↔object は未検証のまま）
- **Step 3（直接対応）**: 昇格条件 4(a) — machine-TV の比較対象を
  LLVM IR 再導出モデルに切り替える設計（大きい。別メモ化を推奨）
- **Step 4（relocation 全件照合）— 実装済み**: §3.3 の reloc レコードを
  全セクション・全種別で emit（`.eh_frame` 等の非コード reloc を含む）。
  PC32/PLT32 は exe 側セクション内で `target + addend − place` を満たす
  スロットを探索して解決（関数外のため入力セクション→exe ベースは
  関数マッピングから推定）。解決は一意の未占有スロットのみに限り、
  位置は `applied_offset` として記録する（複数候補は偶然一致を
  見誤る余地があるため未解決のまま unchecked）。その他種別は
  `elf:<r_type>` の種別番号つきで `resolved_value` なし記録。
  検査項目 `relocation_applied_values` がコード reloc は正確な
  サイト、非コードは記録された `applied_offset` のフィールドで
  exe 適用値と照合（不一致→inconsistent、未解決→unchecked）。
  照合不能時の fallback parse（`object_translation_parse`）も
  全セクションを列挙する

## 8. 未解決の設計問い

- `hir → llvm_ir` の `unchecked_not_translation_validated` 自体をどう
  昇格するか（object 段とは別の問題。machine-TV を IR モデル比較に
  切り替える Step 3 が部分的に解くが、HIR 段の fingerprint 検証は別途必要）
- object 段の relation を profile 条件付きで emit するか、
  `native-bundle` 文脈専用の contract 拡張にするか — **解決**:
  profile 条件付き emit を採用。`SemanticContract::emit_native_object_transformations`
  が `source_atom→machine_code`（`checked_source_machine_tv_within_profile`）、
  `llvm_ir→object_code`（`unchecked_not_translation_validated` のまま）、
  `object_code→linked_executable`（`checked_object_link_correspondence`）を
  追加する。呼び出しは `prepare_source`/`prepare_module_source` の
  native-bundle 経路のみ（通常 `verify`/sidecar 抽出は従来の6段のまま、
  fail-closed）。`output_identity` は `None` — artifact digest は
  descriptor の `object_translation.transformations` が pin 済み。
  `checked_*` は transformation.validation の値（手法の記述）で
  verdict・昇格語彙ではない。観測面: descriptor の
  `object_translation.transformations` に `source_atom→machine_code`
  を追加して3段すべてを出力し（machine code はメモリ評価のみで
  digest なし）、link 辺は手法名 `validation` と結果 `result` を
  分離した。prepare 系の contract オブジェクトは digest 化のみに
  使われ単体ではシリアライズされないため、emit の観測面は
  descriptor 側が担う
- 外部 relocation（libc import）の assumption 語彙を
  `machine_code_callee_saved_preservation_assumed_for_external_callees` と
  共通化するか — **解決**: 名前の統一は不適切（あちらは callee-saved
  レジスタ保存の仮定、relocation は loader 解決の仮定で別の主張）。
  代わりに assumption カテゴリを共有: レコードの relocation エントリに
  `assumption: "assumed_external_callee"` を構造フィールドとして emit
  し、detail 文字列の `assumed-external` 表記を正準化する
  （checks リスト・verdict・昇格語彙は不変）
