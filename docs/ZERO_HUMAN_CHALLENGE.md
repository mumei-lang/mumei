# SI-1: Zero-Human Challenge — コンパイラ側ドキュメント

> mumei コンパイラの形式検証基盤が、AI 自律生成コードの品質をどのように保証するか。

## 概要

Zero-Human Challenge は、mumei-agent が人間の介入なしに形式検証済みコードを生成できることを実証するイニシアチブである。本ドキュメントは **mumei コンパイラ側** から見たチャレンジの意義と、検証済み資産としての成果物の位置づけを記述する。

エージェント側の詳細なチャレンジ仕様・実行方法・結果については、mumei-agent リポジトリのドキュメントを参照:

- [mumei-agent `docs/ZERO_HUMAN_CHALLENGE.md`](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/ZERO_HUMAN_CHALLENGE.md)

---

## コンパイラから見たチャレンジの意義

### 1. 形式検証パイプラインの実用性証明

mumei の `verify` コマンドは Z3 SMT ソルバーを使い、`requires`/`ensures` 契約の充足可能性を数学的に証明する。Zero-Human Challenge は、このパイプラインが **AI エージェントのフィードバックループに組み込み可能** であることを実証する:

```
mumei-agent (LLM) → .mm コード生成
       ↓
mumei verify --json → Z3 による形式検証
       ↓
構造化フィードバック (JSON) → エージェントへのフィードバック
       ↓
自己修復 → 再検証 → ... → 検証成功
```

`mumei verify --json` の構造化出力（violation type, unsat core, counterexample）が、AI エージェントの自己修復に十分な情報を提供することが鍵となる。

### 2. 効果システムとリソース階層の検証

チャレンジ課題は mumei の高度な検証機能を活用する:

| 機能 | 課題 | 検証内容 |
|------|------|---------|
| 精緻型 (Refinement Types) | safe_queue, bounded_queue | `i64 where v >= 0` のような型レベル制約 |
| 事前/事後条件 | 全課題 | `requires`/`ensures` による契約 |
| 効果システム (Effects) | verified_json_validator | `SafeFileRead(path)` による capability security |
| リソース階層 | deadlock_free_pc | `resources: [buffer, mutex]` による deadlock-free 証明 |
| 含意条件 (Implications) | safe_queue, deadlock_free_pc | `len == 0 => result == 1` のような条件付き保証 |

### 3. 検証済み標準ライブラリとの連携

Zero-Human Challenge の課題は、mumei の検証済み標準ライブラリ (vStd) と密接に関連する:

| チャレンジ | 関連 vStd モジュール | 関係 |
|-----------|---------------------|------|
| safe_queue | `std/container/safe_queue.mm` (vStd-3) | 手書き参考実装 — AI 生成コードと等価な正しさを持つべき |
| verified_json_validator | `std/effects.mm` | エフェクトシステムの基盤を使用 |
| deadlock_free_pc | (新規) | リソース階層パターンの新規適用 |

AI エージェントが生成するコードと、人間が書いた `std/container/safe_queue.mm` が **同等の正しさ保証** を持つことは、mumei の検証基盤の一貫性を示す。

---

## チャレンジ課題一覧

### 課題 1: Safe Queue — 100% 安全なキュー

`enqueue`/`dequeue` の overflow/underflow 防止を形式的に証明する 4-atom モジュール。

`std/container/safe_queue.mm` の手書き実装と同パターンの契約を持ち、AI が自律的に同等品質のコードを生成できることを検証する。

**Z3 が証明する性質**:
- `enqueue(len, cap)` で `len < cap` なら `result == len + 1 && result <= cap` (overflow 不可能)
- `dequeue(len)` で `len > 0` なら `result == len - 1 && result >= 0` (underflow 不可能)
- `is_empty(len)` と `is_full(len, cap)` は常に `{0, 1}` の範囲 (Boolean 不変量)

### 課題 2: Verified JSON Validator — FFI + Capability Security

`SafeFileRead` エフェクトを使い、`/tmp/` 配下のファイルのみ読み取りを許可する JSON バリデータ。

**コンパイラが検証する性質**:
- エフェクト `SafeFileRead(path)` の宣言と使用の一致
- `starts_with(path, "/tmp/")` と `not_contains(path, "..")` による capability 境界
- 結果の Boolean 範囲 (`result >= 0 && result <= 1`)

### 課題 3: Deadlock-Free Producer-Consumer — リソース階層

`buffer` と `mutex` の 2 リソースを使い、優先度順序によるデッドロックフリーを証明する 4-atom モジュール。

**コンパイラが検証する性質**:
- `produce`/`consume` が `mutex_held == 0` を前提条件とする（リソース順序の遵守）
- バッファの overflow/underflow 防止
- `buffer_available`/`buffer_has_items` の Boolean 不変量

---

## 検証済み資産としての成果物

Zero-Human Challenge で生成・検証されたコードは、以下の位置づけを持つ:

### 成果物の分類

| 成果物 | 場所 | 意味 |
|--------|------|------|
| 生成コード (`.mm`) | `mumei-agent/examples/challenges/results/*/output.mm` | AI が自律生成した mumei ソースコード |
| 検証ログ | `mumei-agent/examples/challenges/results/*/log.jsonl` | 生成・検証・修正の全ステップ記録 |
| メトリクス | `mumei-agent/examples/challenges/results/*/metrics.json` | 試行回数、成功率、所要時間 |
| 参考実装 | `mumei/std/container/safe_queue.mm` | 手書きの検証済み標準ライブラリ |

### 「検証済み」の意味

mumei における「検証済み」は以下を意味する:

1. **数学的保証**: Z3 SMT ソルバーが `requires` → `ensures` の含意を証明（反例が存在しないことの証明）
2. **契約の完全性**: すべての atom の事前条件・事後条件が検証される
3. **効果の追跡**: エフェクト付き atom は、宣言されたエフェクトのみを使用することが保証される
4. **リソースの安全性**: リソース階層が循環参照を含まないことが保証される

これは「テストが通った」レベルの保証ではなく、**すべての可能な入力に対する正しさの数学的証明** である。

---

## 実行方法

チャレンジの実行自体は mumei-agent リポジトリで行う:

```bash
# mumei-agent リポジトリで:

# Dry-run（spec バリデーションのみ、mumei バイナリ不要）
python -m examples.challenges.run_challenge --all --dry-run

# フル実行（mumei バイナリ + LLM API キー必要）
python -m examples.challenges.run_challenge --all --log-dir examples/challenges/results/
```

GitHub Actions の `workflow_dispatch` でも実行可能。詳細は [mumei-agent の ZERO_HUMAN_CHALLENGE.md](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/ZERO_HUMAN_CHALLENGE.md) を参照。

---

## 今後の展望

### コンパイラ側の拡張

- **Temporal effects**: 時間的なエフェクト制約（例: `timeout(5s)` 付きの操作）のチャレンジ
- **Multi-file verification**: 複数ファイルにまたがるモジュール間検証
- **Incremental verification**: 変更差分のみの再検証による高速化
- **Proof certificate export**: 検証証明書の外部フォーマット（JSON-LD 等）エクスポート

### vStd との統合

チャレンジで生成されたコードのうち、品質基準を満たすものは vStd (Verified Standard Library) に昇格させることを検討:

1. AI 生成コード → Zero-Human Challenge で検証
2. 検証済みコード → 人間レビュー（コードスタイル・API 設計）
3. 承認 → `std/` ディレクトリに統合

これにより、AI 生成コードが標準ライブラリの一部として信頼できる資産になるパスが開ける。

---

## 関連ドキュメント

- [mumei-agent `docs/ZERO_HUMAN_CHALLENGE.md`](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/ZERO_HUMAN_CHALLENGE.md) — エージェント側のチャレンジ詳細
- [`docs/CROSS_PROJECT_ROADMAP.md`](CROSS_PROJECT_ROADMAP.md) — クロスプロジェクトロードマップ (SI-1 セクション)
- [`docs/ROADMAP.md`](ROADMAP.md) — mumei コンパイラロードマップ
- [`std/container/safe_queue.mm`](../std/container/safe_queue.mm) — 検証済みキューの参考実装
- [`docs/CAPABILITY_SECURITY.md`](CAPABILITY_SECURITY.md) — Capability Security の設計ドキュメント
- [`docs/CONCURRENCY.md`](CONCURRENCY.md) — 並行処理とリソース階層のドキュメント
