---
layout: default
title: "Onboarding Guide — Mumei"
description: "Start using Mumei from existing code, natural-language specifications, or direct .mm modules, then migrate toward formal verification."
keywords: "mumei onboarding, formal verification, existing code audit, mumei-agent, proof-driven programming"
---


# Mumei Onboarding: from existing code to `.mm`

Mumei can start from existing code or natural-language requirements, then gradually move the most important logic into `.mm`. Use this path when you want formal feedback before committing to a full `.mm` rewrite.

## Canonical vocabulary

Follow `docs/CROSS_PROJECT_ROADMAP.md` as the top-level contract. User-facing onboarding uses only `harness_contract`, `intent_fidelity`, `artifact_paths`, `budget_policy_fingerprint`, and `lean_verified` for mumei-side evidence. The no-`.mm` path keeps the mumei-agent names `spec_health_issues`, `verification_violations`, `cross_validation_gaps`, `next_steps`, `migration_hints`, `healed_files`, and `heal_errors`; do not rename them when handing users from `audit` / `scan_and_fix` into generated `.mm` verification.

`lean_verified` is not a generic success label. Z3 `unknown` atoms may escalate to mumei-lean, but Mumei accepts the result as proven only when callers opt in with `--allow-lean-verified` and the atom certificate plus Lean metadata both carry the current `translator_version` and `bridge_lemma_hash`; otherwise CLI, MCP consumers, and certificate verification must treat the atom as stale/unproven.

## First path: audit before `.mm`

Use `mumei-agent audit --code-file ... --auto-migrate --auto-heal` or MCP `scan_and_fix` before asking a new user to author `.mm`. Both entrypoints must present the same gate order and the same names:

1. `audit` emits `spec_health_issues`, `verification_violations`, `cross_validation_gaps`, and `next_steps`.
2. `migrate-suggest` / `--auto-migrate` emits `migration_hints` and generated `.mm` skeleton paths.
3. `heal` / `--auto-heal` emits `healed_files` and `heal_errors`.

User-facing wording is fixed to: "既存コードを渡すだけでバグ箇所を指摘", "仕様から既存コードとの差分を指摘", and "仕様単独でおかしい場合を指摘". `next_steps` is the handoff to human review; do not create synonyms for the issue buckets or for the post-audit keys `migration_hints`, `healed_files`, and `heal_errors`.

V1 implementation order is fixed: `V1-A` spec health and `V1-B` code audit can proceed in parallel, then `V1-C` spec→code and `V1-D` code→spec conformance, then `V1-E` human review. V1-E is only the `next_steps`-origin review flow; do not expose `recommendations`, `review_actions`, or other aliases as user-facing contract keys. Lean work is only the Z3-`unknown` complement and must not become a general audit path.

When `scan_and_fix` receives a spec, read `audit`, `spec_alignment`, and `conformance_verification` as separate views. `audit` owns the no-`.mm` buckets and migration/heal artifacts, `spec_alignment` owns spec↔code gaps, and `conformance_verification` owns traceability plus the next_steps-first human/markdown report. For the explicit V1-C/V1-D bidirectional summary, use mumei-agent `verify-traceability` or MCP `verify_code_spec_traceability`; they keep `conformance`, `drift`, `cross_validation_gaps`, `drift_score`, and `next_steps` as fixed keys.

To see the no-`.mm` route before writing `.mm`, run the Phase 7 Spec-Code Verification Suite in [`mumei-demo`](https://github.com/mumei-lang/mumei-demo/tree/main/scenarios/spec_code_verification_suite): `make demo-spec-code`. It bundles V1-A spec health, V1-B code audit, V1-C spec→code conformance, and V1-D code→spec drift while keeping `next_steps` as the only human-review entrypoint.

## Step 0: `.mm`を書かずにバグ指摘を受ける

まずは既存コードや仕様をそのまま渡し、バグ候補・仕様ドリフト・境界条件漏れを洗い出します。

```bash
# 既存コードを渡すだけ
uv run mumei-agent audit --code-file payment.py --auto-migrate --auto-heal
uv run mumei-agent validate-code --input payment.py --language python
printf '%s\n' "残高不足の場合はエラーを返す" > spec.txt
uv run mumei-agent validate-spec-to-code --spec spec.txt --code payment.py --language python
```

`audit --auto-migrate --auto-heal` and MCP `scan_and_fix` are the canonical no-`.mm` route. Read their artifacts as `spec_health_issues`, `verification_violations`, `cross_validation_gaps`, `next_steps`, `migration_hints`, `healed_files`, and `heal_errors`. `cross_spec.json.contract_consistency[]` maps to agent `missing_constraints[]`; `global_invariant_conflicts[]` maps to `divergences[]`; `circular_dependencies[]` maps to `drift_issues[]`.

この段階では `.mm` ファイルは不要です。出力された counterexample、仕様とコードの不一致、足りない事前条件を移行バックログとして扱います。
`audit` の出力に `verification_violations` や `cross_validation_gaps` が含まれる場合は、`next_steps:` フィールドも確認してください。各項目は、次に実行すべき確認コマンド、仕様・コードの修正候補、または `.mm` 移行前に解消すべきギャップを示します。

## Step 1: 自然言語からスペックを生成して検証

自然言語の意図を JSON spec に落とし、そこから `.mm` を生成して Mumei の verifier に渡します。

```bash
uv run mumei-agent extract-spec --text "安全な銀行送金" --output spec.json
uv run mumei-agent generate --spec-file spec.json --output transfer.mm
mumei verify transfer.mm
```

`mumei verify transfer.mm` を実行するには、Mumei CLI の `mumei` バイナリが必要です。まだインストールしていない場合は、先に `curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash` を実行してください。

この時点の `.mm` は最初の候補です。検証エラーは「仕様が強すぎる」「境界条件が足りない」「Z3 の決定可能断片を超えている」のどれかとして分類し、必要に応じて spec か実装を修正します。

## Step 2: 生成された `.mm` をレビュー・修正

生成された `.mm` は、人間がレビューしながら小さく修正します。

- LSP (`mumei lsp`) を使い、エディタ上で diagnostics、hover、補完、定義ジャンプを確認する。`.mm` 内の `/// spec:` は `mumei-agent` の `spec_health_issues` として該当コメント行に出し、`.py` / `.rs` / `.go` では `verification_violations` / `cross_validation_gaps` をインライン表示する。
- REPL (`mumei repl`) を使い、小さい式や atom をインタラクティブに検証する。自然言語仕様は `:verify-spec <path|inline>`、他言語コードは `:verify-code <path>` で `mumei-agent` の `spec_health_issues` / `verification_violations` / `cross_validation_gaps` / `next_steps` を確認できる。
- `uv run mumei-agent check-spec-health transfer.mm` で矛盾、到達不能な `requires`、過剰拘束、曖昧な postcondition を確認する。
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
