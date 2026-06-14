# Mumei Onboarding: from existing code to `.mm`

Mumei can start from existing code or natural-language requirements, then gradually move the most important logic into `.mm`. Use this path when you want formal feedback before committing to a full `.mm` rewrite.

## Step 0: `.mm`を書かずにバグ指摘を受ける

まずは既存コードや仕様をそのまま渡し、バグ候補・仕様ドリフト・境界条件漏れを洗い出します。

```bash
# 既存コードを渡すだけ
python -m agent audit --code-file payment.py
python -m agent verify-foreign --file payment.py --language python
printf '%s\n' "残高不足の場合はエラーを返す" > spec.txt
python -m agent validate-spec-to-code --spec spec.txt --code payment.py --language python
```

この段階では `.mm` ファイルは不要です。出力された counterexample、仕様とコードの不一致、足りない事前条件を移行バックログとして扱います。

## Step 1: 自然言語からスペックを生成して検証

自然言語の意図を JSON spec に落とし、そこから `.mm` を生成して Mumei の verifier に渡します。

```bash
python -m agent extract-spec --text "安全な銀行送金" --output spec.json
python -m agent generate --spec spec.json --output transfer.mm
mumei verify transfer.mm
```

この時点の `.mm` は最初の候補です。検証エラーは「仕様が強すぎる」「境界条件が足りない」「Z3 の決定可能断片を超えている」のどれかとして分類し、必要に応じて spec か実装を修正します。

## Step 2: 生成された `.mm` をレビュー・修正

生成された `.mm` は、人間がレビューしながら小さく修正します。

- LSP (`mumei lsp`) を使い、エディタ上で diagnostics、hover、補完、定義ジャンプを確認する。
- REPL (`mumei repl`) を使い、小さい式や atom をインタラクティブに検証する。
- `python -m agent check-spec-health transfer.mm` で矛盾、到達不能な `requires`、過剰拘束、曖昧な postcondition を確認する。
- verifier の counterexample を仕様レビューの単位にし、1 回の修正で 1 つの失敗原因だけを潰す。

レビュー時は、仕様を証明しやすい形へ寄せることを優先します。特に配列境界、有限範囲の量化子、線形算術、明示的な状態遷移は [`SPEC_GUIDE.md`](SPEC_GUIDE.md) の推奨形に合わせます。

## Step 3: `.mm` を直接書く

重要な仕様面が安定したら、新規ロジックを直接 `.mm` で書きます。

```mumei
atom transfer(balance: i64, amount: i64)
requires: balance >= 0 && amount > 0 && amount <= balance;
ensures: result == balance - amount && result >= 0;
body: balance - amount;
```

設計の基本は次の通りです。

- `requires`: 呼び出し側が満たすべき入力条件を書く。配列 index、残高、状態などの境界はここで明示する。
- `ensures`: atom が返す値や副作用後の状態について、検証したい保証を書く。
- `effects`: 外部リソースや temporal state を使う場合、有限状態と明示的 transition で表す。
- 決定可能断片（P8-D）の範囲内で書く。線形算術、単一 index の配列 read/write、bounded `forall`、有限状態機械を優先し、乗算/除算/mod、量化子交代、暗黙の履歴制約は Lean escalation 候補として扱う。

詳しい spec-writing pattern、アンチパターン、テンプレートは [`SPEC_GUIDE.md`](SPEC_GUIDE.md) を参照してください。
