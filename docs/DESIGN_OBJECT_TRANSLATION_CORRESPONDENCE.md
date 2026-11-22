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
```

注意: `SemanticContract` 本体の変更は a672 の C03 レーンに属するため、
フィールド追加・`relation`/`validation` 語彙の確定は親セッション経由で
調整する（本メモは提案のみ）。

### 3.2 検証単位と入出力 identity

- **検証単位**: LLVM IR 関数 ↔ object `.text` シンボル ↔ linked executable の
  同名シンボルの 3 点対応を関数ごとに記録する。
- **入出力 identity**:
  - 入力: 固定済み `llvm_ir_sha256`（record ピン）＋ 固定 llc/ld argv
    （`toolchain_object_executable_reproduction` が保証）
  - 出力: `object_sha256`、`executable_sha256`（record `files` エントリ）
  - 対応表: 関数名 → {IR 関数名、object シンボル、セクション、オフセット、
    バイト範囲、exe 内アドレス} — この対応表自体を descriptor に含めると
    consumer（mumei-agent `native-object-recheck`）が部分検証できる

### 3.3 relocation 記録

`object_link_corresponds` が比較するのは関数バイトと call 解決先のみ。
producer は object の relocation エントリを列挙し、各 reloc の
{ターゲットシンボル、addend、適用後の exe 内値} を emit 対象とする。
外部シンボル（libc/runtime stub）は「前提」として区別し、
`machine_code_callee_saved_preservation_assumed_for_external_callees` と
同じ assumption カテゴリに入れる。

## 4. `unchecked_not_translation_validated` からの昇格条件

昇格は「全条件を満たす or 未充足条件が `checks_not_performed` として
列挙される」のどちらかでのみ許可（部分昇格なし）:

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
     OutsideProfile が残らないことを昇格ゲートとする（推移的主張:
     source↔machine が成立し LLVM エミッタの确定性が toolchain pin で
     担保されるなら IR↔object も一致、とする縮約。弱いが実装コスト小）
   - (a) が本筋、(b) は中間段階の有界クレームとして許容
5. **未モデル化残域の列挙**: `unsupported_instruction`・`call_arguments`・
   `frame_memory_bounds`・over-bound ループ・MXCSR/NaN・外部呼び出し前提が
   descriptor の `checks_not_performed` / `functions.<name>.outcome` に
   全件残っていること（昇格対象外の残域を沈黙させない）
6. **非決定性の排除**: 同一入力から同一 digest が出ること（toolchain pin +
   `toolchain_object_executable_reproduction` で担保済み。再ビルドで
   digest が変わった場合は昇格失敗）
7. **fail-closed**: 上記いずれかが判定不能なら inconclusive/not_performed で
   終了し、昇格した validation 値を書かない

昇格後の `validation` 語彙候補: `checked_within_machine_tv_profile`
（(b) の推移的主張）または `checked_object_translation_tv`（(a) の
直接対応）。既存の `compiler_local_fingerprint_not_equivalence` と
`not_executed_by_extractor` の中間に位置する第3の honesty レベルとして
導入を検討。

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
mumei native-bundle recheck-machine-tv bundle loop.mm --expected-build-id <id> --json
# → functions.countdown.outcome = "equivalent_within_profile"
#   checks_performed に "machine_code_bounded_loop_function_tv"
#   checks_not_performed に "machine_code_tv_loop_executions_beyond_bound"
```

昇格判定の計測に使える最小ケース:

1. **全等価ケース**: 全関数が `equivalent_within_profile` の bundle
   （単純な i64 atom 2-3 個 + main）— 昇格条件 1-4 が全て満たされるかを確認
2. **out-of-profile 混入ケース**: over-bound ループまたは未対応命令を含む
   atom を混ぜ、`functions.<name>.outcome` に OutsideProfile が残り
   昇格が抑止されることを確認
3. **relocation 破壊ケース**: object の call target を1バイト書き換えた
   bundle で `object_link_correspondence` が `Different` → 昇格失敗を確認
4. **非決定性ケース**: 同一ソースを2回 build して object_sha256 が一致するか
   （不一致なら条件6で昇格失敗）

## 7. 段階的ロードマップ

- **Step 1（メタデータのみ）**: transformations に `llvm_ir→object_code`・
  `object_code→linked_executable` を `validation: "not_executed_by_extractor"`
  で追加し、output_identity に record pin の digest を入れる。
  検証は増えないが変換関係が消えない
- **Step 2（推移的昇格）**: 昇格条件 4(b) のゲートを producer に実装 —
  全関数 equivalent + 残域列挙の確認で `checked_within_machine_tv_profile`
  を emit
- **Step 3（直接対応）**: 昇格条件 4(a) — machine-TV の比較対象を
  LLVM IR 再導出モデルに切り替える設計（大きい。別メモ化を推奨）
- **Step 4（relocation 全件照合）**: §3.3 の reloc レコードを emit +
  exe 内解決値との全件照合

## 8. 未解決の設計問い

- `hir → llvm_ir` の `unchecked_not_translation_validated` 自体をどう
  昇格するか（object 段とは別の問題。machine-TV を IR モデル比較に
  切り替える Step 3 が部分的に解くが、HIR 段の fingerprint 検証は別途必要）
- object 段の relation を profile 条件付きで emit するか、
  `native-bundle` 文脈専用の contract 拡張にするか
- 外部 relocation（libc import）の assumption 語彙を
  `machine_code_callee_saved_preservation_assumed_for_external_callees` と
  共通化するか
