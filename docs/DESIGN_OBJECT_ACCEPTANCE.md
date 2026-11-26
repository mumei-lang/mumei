# Design memo: object-translation acceptance producer

Status: design memo (no implementation yet). Audit backlog item 4 —
agent 側に `native-object-accept` ルート（`object_source_machine_code_correspondence`
→ `object_translation_correspondence` 対応づけ、受理メタデータのみ消費）が
計画されているが、Core 側には「object コードを受理 evidence に昇格させる」
記述子を emit する producer がまだない。本メモは既存資産の棚卸し、受理が
要求すべき条件、昇格後語彙、段階案をまとめる。

前提となる整理: `mumei native-bundle accept-object` は #1228 で既に
実装済みだが、これは machine-TV slice の受理であって object-translation
の受理ではない。`accepted` 判定は `machine_tv_acceptance_decision`
（source-atom 関数の `Outcome::EquivalentWithinProfile` 等）だけを見て
おり、同一記述子内に埋め込まれる `object_translation.correspondence.checks`
は記録されるだけで受理をゲートしない。つまり
`object_data_correspondence: inconsistent` や
`object_symbol_attribute_correspondence: unchecked` のままでも
`accepted: true` が出る — これが本メモが埋める具体的ギャップ。

## 1. 目的とスコープ

- 目的: 再現された object コードを「object 翻訳段（`llvm_ir → object_code`
  とその補助対応関係）の受理 evidence」に昇格させる acceptance
  descriptor の設計。`mumei.*acceptance/v1` 系の既存 schema に乗せ、
  agent `native-object-accept` ルートが消費できる形にする。
- スコープ:
  - correspondence 面（`correspondence.checks` の全項目）を受理判定に
    接続する
  - 受理条件の列挙（どの項目が全て `checked` のとき acceptance を
    名乗れるか）と fail-closed 境界（inconsistent → reject、
    unchecked/unknown → inconclusive）の定義
  - 受理の意味論（何を信頼するか / 何を信頼しないか）の正直な記述
- スコープ外:
  - audit item 5「full object TV」（研究領域。relocation の全数照合や
    任意 IR 拡張ではなく、未検証命令選択の形式的証明）
  - ABI/runtime/OS セマンティクス、観測面（exit status / stdout / stderr）
    の受理 — それぞれ別 producer / 別メモ
  - verdict / grade / checks-list promotion（keep-off 方針のまま）
  - producer authenticity / historical execution の主張
  - Step 4（liftable subset の拡大）— #789 の mixed register pair のみ
    部分解凍、受理条件の拡張は保留

## 2. 現状の棚卸し（再利用可能な機構）

| 機構 | 内容 | 位置づけ |
| --- | --- | --- |
| `mumei native-bundle accept-object`（#1228） | bundle 再現 + symbol/call-edge/link 再検査 + bounded machine-TV を走らせ、`mumei.native-evidence-acceptance/v1` を emit | 実装済み。受理 claim は machine-TV slice — 移行前の emit scope は `native_bundle_object_machine_code_translation_correspondence[_without_object_link]`、Step 3 以降は `native_bundle_object_translation_correspondence[_without_object_link]` を emit（現行出力は後者） |
| `machine_tv_acceptance_decision` | `Outcome` 集合を `Accepted` / `Different` / `Unknown` に写像（Different→reject、Unknown→inconclusive、全件 equivalent で accepted） | 受理判定関数のパターンとして再利用（純粋関数で単体テスト可能） |
| `write_object_machine_tv_acceptance` | JSON/text 出力、build_id・llvm_sha256・object_sha256 バインド、checks_performed / checks_not_performed 列挙 | 記述子 emit 経路として再利用 |
| `report_object_acceptance_inconclusive` | inconclusive 時は記述子を emit せず終了コード 3 | fail-closed 境界の既存実装 |
| `object_translation_json` + `object_translation_correspondence_json` | transformations 辺の result/validation + `correspondence.checks` 項目列（`{item, result, detail}`）を記述子に埋め込み | 受理判定の入力として既に存在 |
| `correspondence.checks` の項目（10+） | `function_coverage` / `function_count` / `llvm_ir_object_equivalence` / `relocation_targets` / `relocation_bounds` / `relocation_resolved_values` / `relocation_applied_values` / `hir_llvm_model_correspondence` / `object_data_correspondence` / `object_symbol_attribute_correspondence` | 昇格アークで揃った対照項目。受理を名乗るために必要な全項目はここに揃う |
| `llvm_ir_object_edge_result` + `LLVM_IR_OBJECT_GATE_ITEMS` | 辺 result 集約（全項目 checked → `checked_correspondence`、1件 inconsistent → `inconsistent_correspondence`、1件 unchecked → `unchecked_not_translation_validated`） | 受理判定と同じ3値集約ルールとして再利用 |
| `object_symbol_correspondence` / `object_call_edge_correspondence` / `object_link_correspondence` | 構造的なシンボル・呼出辺・リンク配置の対照（hard gate） | 受理の前提条件として既存（これらが fail したら記述子自体が出ない） |
| canonical argv pin（`canonical_link_argument_vector_compared`） | リンクの再現決定性を固定 | 受理条件の一部として既存 |
| saved-evidence recheck の考え方 | pinned bundle から再導出して証跡（build_id, sha256）を照合 | 消費側パターンとして再利用。`evidence::decode` 自体は `mumei.native-evidence/v1` 専用で受理記述子 schema（`mumei.native-evidence-acceptance/v1`）は読めない — 受理記述子側のデコードは別途必要（消費は agent 側の想定） |
| e2e harness（`accept_object_emits_bounded_descriptor` 等） | accept-object の JSON/exit-code 検証 | 新受理条件の e2e パターンとして再利用 |

## 3. 欠けている部分

1. **受理判定が correspondence 面を消費していない（本題）**。
   `machine_tv_acceptance_decision` は機械語 TV outcome のみを見る。
   `llvm_ir_object_equivalence`・`object_data_correspondence`・
   `object_symbol_attribute_correspondence`・relocation 4項目・
   `hir_llvm_model_correspondence` が `unchecked`/`inconsistent` でも
   `accepted: true` が出うる。到達可能な具体例: solver が
   判定不能な関数（`solver_unknown`）を含む bundle では
   `llvm_ir_object_equivalence` が `unchecked` のまま、
   `.eh_frame` 系など bounded 解析が解決できない再配置を含む
   bundle では relocation 項目が `unchecked` のまま、それぞれ
   機械語 TV は素通しで `accepted: true` が emit される。
   （`inconsistent` の受理は byte 同一の再現検査が先に止まる
   ため実際には到達しにくく、項目の役割は defense-in-depth。）
2. **object-translation 受理を名指す語彙がない**。scope は
   machine-code TV に scoped された名前だけで、「再現 object が
   pinned LLVM IR に対応する」claim を表す値がない。
3. **`checks_performed` が対照項目を列挙しない**。今は
   `object_symbol_correspondence` / `object_call_edge_correspondence` /
   `object_link_correspondence` 等の hard gate 名のみで、
   `llvm_ir_object_equivalence` 等の対照項目名は含まれない。
   agent が受理内容を discharge するとき、項目単位で名前を渡せない。
4. **agent 対応づけが古いまま**。agent ルートの計画は
   `object_source_machine_code_correspondence` →
   `object_translation_correspondence` の1対1写像だったが、
   correspondence 面は10項目以上に拡張済み。写像の対象は
   項目集合（あるいは集約 status）に見直す必要がある。
5. **受理の意味論が未記述**。object-translation 受理が
   「何を信頼するか（= 全対照項目 checked の object コード）」
   「何を信頼しないか（= llc 自体の正しさ、ABI、ランタイム、
   producer authenticity）」を記述子・docs の両方で明示する必要がある。
6. **producer authenticity / historical execution は引き続き
   not_performed**。既存 acceptance の正直な限界を継承する。

## 4. 受理が要求すべき条件

以下が全て成立したときだけ object-translation acceptance を名乗る。
どれか1つでも `inconsistent` 相当 → reject（終了コード 1、
記述子なし）。`unchecked`/不明 → inconclusive（終了コード 3、
記述子なし）。fail-closed。

1. **構造前提**: `object_symbol_correspondence`・
   `object_call_edge_correspondence` が checked（既存の hard gate で
   保証）。`object_link_correspondence` は link 再現が走ったとき
   checked が必須、link 未検査のときは not_performed として
   scope に `_without_object_link` suffix を付けて受理範囲を
   明示（既存 convention と同じ）。
2. **対照項目の集約**: 判定が読むのは
   `object_translation.correspondence` の項目集約
   （`correspondence.status`）であって、`llvm_ir→object_code` 辺の
   result（`checked_llvm_ir_object_model_correspondence` 評価）
   ではない。後者は link 対照と canonical argv を内部で要求する
   ため、`_without_object_link` では立たない —— 項目集約は
   link 非依存なので両 scope で使える。集約に含まれる全項目
   （`function_coverage` / `function_count` /
   `llvm_ir_object_equivalence` / relocation 4項目 /
   `hir_llvm_model_correspondence` / `object_data_correspondence` /
   `object_symbol_attribute_correspondence`）が `checked` で
   受理条件成立。`inconsistent` が1件でも → reject、
   `unchecked`/`unknown` が1件でも → inconclusive
   （`solver_unknown` は既に `unknown` として分離済みなので
   ここに来る）。
   境界の正直な記述: relocation 項目は全エントリの解決を要求
   するので、`.eh_frame` 系など bounded 解析が値を解決できない
   再配置を含む bundle は集約が `unchecked` のまま inconclusive
   —— 受理できない。これは意図した fail-closed 境界で、
   例外を入れて受理を広げない。
3. **再現条件**: `fresh_same_profile_source_recheck`・
   `llvm_snapshot_translation_recheck`・
   `toolchain_object_executable_reproduction` が performed かつ成功。
   `pinned_member_merge_rederivation` は複数ファイル bundle の
   場合のみ必須（単一ファイル bundle では項目自体が存在しない）。
   `canonical_link_argument_vector_compared` は無条件必須 ——
   pinned argv が記録されているなら比較が実行され一致している
   こと、pin 自体が無い bundle はリンク再現の決定性を名乗れず
   inconclusive。この要件は `object_link_correspondence` の可否と
   独立 —— `_without_object_link` が外すのはリンク対照項目
   だけで、argv 比較は引き続き要求する（relink は
   `_without_object_link` scope でも走る）。
4. **machine-TV**: `machine_tv_acceptance_decision` が `Accepted`。
   object-translation 受理は machine-TV 受理の superset ——
   「再現 object が pinned LLVM IR に対応する」に加えて
   「source ↔ machine 等価が profile 内で立つ」を併せて受理する。
   （機械語 TV だけを受理したい利用者向けに machine-TV slice の
   受理条件を緩めない。）

### 受理の意味論（正直な記述）

受理 = 「再現された object コードが、pinned LLVM IR と、この bounded
correspondence profile で立証可能な範囲で対応している」。
具体的に信頼するのは:

- liftable subset 内関数について solver が立証した命令選択・
  レジスタ割当ての対応（`llvm_ir_object_equivalence`）
- IR データグローバル初期値と object データセクションの対応
  （`object_data_correspondence`、anonymous `unnamed_addr` は
  merged-一致まで）
- IR linkage ↔ object ELF binding の対応
  （`object_symbol_attribute_correspondence`）
- relocation の 4 項目（targets/bounds/resolved/applied）
- リンク配置と canonical argv による再現決定性

信頼しない（not_performed に列挙）:

- llc / assembler / linker 自体の正しさ（toolchain を対象にしない）
- ABI・calling convention・register convention の妥当性
- runtime/loader/CRT/OS セマンティクス・観測面
- producer authenticity（誰が emit したか）、historical execution
- subset 外関数・`solver_unknown` 関数（受理には入らない）

## 5. 受理記述子の語彙案

- **schema**: `mumei.native-evidence-acceptance/v1` を再利用
  （`schema_version` 変更なし）。`acceptance: "evaluated"`,
  `accepted: true` の立て方は既存通り。
- **scope**: `native_bundle_object_translation_correspondence`
  （新規値。`_without_object_link` suffix convention は継承）。
  machine-TV slice の既存 scope
  （`native_bundle_object_machine_code_translation_correspondence`）
  は語彙として維持する — 過去に emit された記述子を解釈する
  ための値として残る。最終的に `accept-object` は新 scope だけを
  emit する（dual-emit・切替 flag なし）。Step 2 の判定接続
  時点では旧 scope のまま emit し（厳しい方向のみの変更なので
  既存 consumer は拒まない）、Step 3 で agent 側の準備が整って
  から emit を新値に切り替える。
  影響の正直な記述: 受理の主張は強くなるが受理対象は狭まる。
  unchecked 項目を残す bundle は機械語 TV に合格しても新しい
  受理記述子を得られない（inconclusive、exit 3）—— 旧記述子を
  読めることと新記述子を発行できることは別で、machine-TV slice
  だけの受理を新規に得る経路はなくなる。これは意図した
  fail-closed の狭まり。移行順序: 先に agent 側が新旧両 scope
  を受理できるよう allowlist と項目集合写像を更新してから、
  Core が新 scope への emit を切り替える（Core を先に切ると
  旧 agent は正当な新記述子も scope 不一致で拒む空白ができる）。
  旧 scope の記述子を読む経路は互換のため残す。
- **checks_performed**: correspondence 項目名をそのまま列挙
  （`llvm_ir_object_equivalence`、`object_data_correspondence`、
  `object_symbol_attribute_correspondence`、`hir_llvm_model_correspondence`、
  relocation 4項目、`function_coverage`/`function_count`）。
  既存の hard gate 名（`object_symbol_correspondence` 等）と
  `object_source_machine_code_correspondence` は従来通り含める。
- **checks_not_performed**: 従来通り
  producer_authenticity/descriptor_authenticity/historical_execution/
  register_convention_validation/optimizer_semantics/
  abi_runtime_semantics/os_*_observation/runtime_observation_semantics/
  grade_promotion/lean_kernel_checks に加え、object-translation 受理
  固有で skip された項目（`_without_object_link` の場合の
  `object_link_correspondence`）を明記。
- **binding**: `evidence_fingerprint` は `build_id`・`source_sha256`・
  `llvm_sha256`・`object_sha256`・`canonical_argv_hash`・`emit_scope` を
  含む canonical JSON の SHA-256（決定点 f 参照）。
- **agent 対応づけ**: Core は `object_translation_correspondence` と
  いう agent-owned 名を emit しない（既存の方針を継続）。
  agent `native-object-accept` は新 scope と項目名列挙を消費し、
  自分の `object_translation_correspondence` 名に写像する。

## 6. 段階案

- **Step 1**: 受理判定関数 `object_translation_acceptance_decision`
  （correspondence 集合 → `Accepted`/`Inconclusive`/`Different`）を
  純粋関数として実装 + 単体テスト。emit にはまだつなげない。
  （本 PR で実装 — 必須項目名は
  `OBJECT_TRANSLATION_ACCEPTANCE_ITEMS` として固定。）
- **Step 2**: `accept-object` の `accepted` 判定に correspondence
  集約を接続し、checks_performed/not_performed を項目名レベルに
  拡張。machine-TV 受理との superset 関係を docs に記述。この時点の
  emit scope はまだ旧値のまま —— 判定だけが先に厳しくなる
  （受理対象が狭まる方向のみの変更）。旧 scope をピン留めした
  既存テストの期待値は、emit が新値に切り替わる Step 3 の変更と
  同時に更新する（先行すると現行出力と乖離して fail する）。
  （本 PR で実装 — canonical argv 再現と全 correspondence 項目
  checked を受理条件に追加し、`checks_performed` に必須項目名を
  列挙。scope は旧値のまま。）
- **Step 3**: e2e —— 正常受理（全項目 checked → `accepted`）、
  data/symbol-attribute/relocation が unchecked のとき
  inconclusive、`inconsistent` で reject、`_without_object_link`
  scope、`canonical_argv` 欠損で inconclusive、の6ケース。
  agent 側（parent lane）は先に `native-object-accept` を新旧両
  scope と項目集合写像に対応させてから、Core が emit scope を
  新値に切り替える —— scope をピン留めした既存テストの期待値は
  この emit 切替と同じ変更で更新する。逆順だと旧 agent が正当な
  新記述子を scope 不一致で拒む空白ができる。Core 側は合意した
  項目名・scope・schema を docs に確定。
  （本 PR で実装 — emit scope を
  `native_bundle_object_translation_correspondence[_without_object_link]`
  に切替（agent#1323 が両 scope を受理済み）。e2e: 正常受理・
  unchecked→inconclusive・inconsistent→reject・canonical_argv
  欠損→inconclusive は実 fixture で pin。`_without_object_link`
  scope は単体テストで pin —— fresh ELF の fixture から
  `object_link_corresponds` の uncheckable 経路は到達不能なため。
  旧 scope 値は語彙として維持、emit 経路は新値のみ。）
- **Step 4（#789、部分解凍）**: LLVM IR ↔ object の直接比較に flat な
  `{ i64, double }` / `{ double, i64 }` の register 結果を追加し、
  `rax`/`xmm0` のビットパターンを比較する。入れ子の mixed 結果や mixed
  `sret`、FP 演算／比較、既存の deferred セットは対象外のまま。
  `evaluate_machine_tv` もこの結果形状で IR をエンコードできた場合は
  IR の戻り値を比較対象とする。従来の source-model 比較は両値が NaN なら
  payload の違いを区別しないが、IR 比較は payload を含めて一致を要求する。
  そのため IR と object の NaN payload だけが異なる場合も `Different` となり、
  `accept-object` は受理しない。これは FP 演算を含まない IR のビット保存を
  要求するもので、MXCSR や FP 演算時の NaN payload の検証は主張しない。
  IR をエンコードできない場合の source-model 比較への fallback は従来通り。
  受理ゲートの構成、項目名・verdict・grade は変更しない。解凍しない項目の理由は
  [LLVM IR → object の設計メモ §6](DESIGN_LLVM_OBJECT_CORRESPONDENCE.md#6-段階案)
  を参照。

## 7. 決定点（推奨対応つき）

- **(a) `accept-object` 拡張 vs 別コマンド**: 推奨は `accept-object`
  拡張。受理は同じ再現パイプラインの上に乗る claim で、別コマンド
  にすると whole reproduction を二重化する。scope 名を変えるので
  受理の意味論もはっきり分かれる。`accepted` が correspondence
  項目を見ずに立つのは矛盾なので緩めない。machine-TV slice だけ
  の受理経路は残さない —— superset claim に置き換わり、受理は
  厳しくなる方向のみ（旧 scope の記述子は語彙として残る）。
- **(b) 受理に要求する項目集合**: 推奨は「`correspondence.checks`
  内の全項目 checked」＝ `correspondence.status ==
  "checked_correspondence"` と同じ集約ルール。
  部分集合（例: equivalence のみ）で受理すると、欠落項目が
  `unchecked` のまま受理が立つ同じ穴を掘る。
- **(c) scope 値**: 推奨は新規値
  `native_bundle_object_translation_correspondence`
  （machine-TV の scope とは別、`_without_object_link` 継承）。
  既存 scope を変更しない —— 受理意味の変化は値名で表す。
- **(d) canonical argv**: 推奨は無条件必須 —— pinned argv が
  記録されているなら比較実行・一致が必要で、pin 自体が無い
  bundle はリンク再現の決定性を名乗れず inconclusive。この要件は
  `object_link_correspondence` の可否と独立 ——
  `_without_object_link` が外すのはリンク対照項目だけで、
  argv 比較は scope に関わらず要求する。判定の入力は辺の
  result ではなく項目集約なので no-link でも受理条件は立つ。
- **(e) agent-owned 名の emit**: 推奨は継続して emit しない。
  `native-object-accept` が項目名集合から自分の
  `object_translation_correspondence` 名に写像する設計にする。
- **(f) evidence_fingerprint binding**: #1445 では `correspondence`
  全体ではなく、受理時の入力 pin を descriptor に bind する。
  attempt binding 用であり、descriptor 自体の真正性は検証しない。
  canonical JSON の詳細は [VERIFICATION_NATIVE.md](VERIFICATION_NATIVE.md#object-stage-machine-code-acceptance-c10-option-b) 参照。
- **(g) skipped subset の扱い**: 推奨は受理判定には無関係（
  `llvm_ir_object_equivalence` 項目が checked であれば skipped
  関数がいても受理）。ただし `solver_unknown` が1件でもあれば
  item unchecked → inconclusive、は Step 1 で既に fail-closed。
