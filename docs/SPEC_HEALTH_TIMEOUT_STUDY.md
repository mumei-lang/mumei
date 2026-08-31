# spec-health ソルバチェックの硬タイムアウト設計調査（#527 フォローアップ）

> 調査日: 2026-08-31。対象は #527 で導入した `check_with_deadline`（ウォッチドッグ +
> `ContextHandle::interrupt()`）が **リンク先 libz3 が 4.14 未満だと効かない**という
> 既知の限界を、どう解消するかの**非破壊な設計調査**。本ドキュメントは調査成果物であり、
> コンパイラの実装は含まない。

## 0. スコープと結論サマリ

| 候補 | 硬タイムアウトの実効性 | 実測コスト | 判定 |
|---|---|---|---|
| A. チェック単位でソルバを別プロセス化（SMT2 を子 `z3` に渡す） | ✅ 4.8.12 でも打ち切れる（子の `-T:` と親の kill の二重化を実測） | ❌ std 検証全体で **5.05x**（5.664s → 28.626s） | 保留（前提条件を満たすまで既定にしない） |
| B. リンク先 libz3 < 4.14 を起動時に拒否 | ✅ 定義上（古い libz3 では動かさない） | ❌ 本リポジトリの CI 自体が全ワークフローで distro の `libz3-dev` を入れており、自分の CI を落とす | 否 |
| C. 警告 + 開発/CI 側を新しい Z3 に寄せる（推奨） | ⚠️ ソルバ側は変えず、影響範囲を消す方向 | ✅ ほぼゼロ | **推奨** |

**総合結論: C を採り、A は「distro libz3 を一級構成として支える」と決めた場合の逃げ道として
前提条件付きで残す。** 理由は §1 の露出範囲と §2 の実測（とくに SMT2 の往復忠実性の欠陥）にある。

なお `Z3WorkerPool` の再利用は**成立しない**。これは Rust 側のソルバ層ではなく `mcp_server.py`
の MCP 用プールで、リースの単位は `mumei verify` の CLI プロセス全体（`docs/TOOLCHAIN.md`
「MCP parallel verification and cache isolation (P8-F)」）であり、spec-health のチェック単位を
載せる層ではない。

## 1. 露出範囲（誰が実際にハングするのか）

| ビルド | libz3 | 上限が効くか |
|---|---|---|
| 配布リリースバイナリ（Linux gnu / musl） | `--features static-link-z3` でソースからビルドした Z3 を静的リンク | ✅ 効く |
| 配布リリースバイナリ（macOS） | `brew install z3` の libz3 を動的リンク（ビルド時の formula 次第。現状 5.x） | ⚠️ 実質効くが formula 依存 |
| 配布リリースバイナリ（Windows） | 上流プレビルド 5.1.0 を動的リンク（`libz3.dll` を同梱） | ✅ 効く |
| `mumei setup` 済みの環境 | 4.14.1 / 5.1.0 | ✅ 効く |
| distro libz3 にリンクした開発ビルド | Ubuntu 22.04/24.04 で 4.8.12 | ❌ 効かない |
| 本リポジトリの CI のうち mumei をビルドするジョブ | `verify-std` / `stdlib-proof-gate` / `ffi-contract-tests` / `generate-std-certs` / `update-metrics` / `otel-tracing` が `apt-get install libz3-dev` | ❌ 効かない |

`static-link-z3` を渡しているのは Linux のビルドステップだけで、macOS と Windows は新しい Z3 を
動的リンクしている（`.github/workflows/release.yml`）。いずれの経路も 4.14 以上を引くため結論は
変わらないが、根拠は「全ターゲットが静的リンク」ではない。CI 側も全 12 ワークフローではなく、
cargo を走らせる上記 6 ジョブだけが露出している（`release.yml` は上記のとおり対象外、残りは
mumei をビルドしない）。

つまり**エンドユーザーの配布物は影響を受けず、露出しているのは開発ビルドと上記 CI ジョブ** である。
これは「まずソルバ実装を変える」より「開発/CI の Z3 を新しくする」ほうが費用対効果が高い、
という C の根拠になる。

## 2. 候補 A（別プロセス化）の実測

前提として spec-health のチェックはモデルを使わず `SatResult` のみを見る。また
`impl Display for Solver`（`Z3_solver_to_string`）は `declare-fun` を含む自己完結した SMT2 を
出力できるため、原理的には次の形で成立し得る。

```rust
fn check_via_subprocess(solver: &Solver<'_>, z3_bin: &Path, timeout_ms: u64) -> SatResult {
    // stdin = format!("{}\n(check-sat)\n", solver)
    // child = z3 -in -T:<secs>        (親側は timeout_ms + grace で kill)
    // 最後の非空行を sat/unsat/unknown として解釈
}
```

### 2.1 硬タイムアウトは実際に効く

| 経路 | 結果 |
|---|---|
| `z3 -in -T:1`（4.8.12、ハングする非線形ゴール） | 1.010s で `timeout` を返す |
| Rust プロトタイプ（`timeout_ms=1`、親 kill） | 512ms で `Unknown`（`subprocess deadline elapsed after 501 ms; child killed`） |

in-process の interrupt が無視される 4.8.12 でも、**子プロセスなら確実に打ち切れる**。

### 2.2 ただし SMT2 の往復忠実性に実欠陥がある

in-process を正解として 94 チェックを突き合わせた結果:

| 比較 | 件数 |
|---|---|
| `Unsat` → `Unsat` | 4 |
| `Sat` → `Sat` | 75 |
| `Unknown` → `Unknown` | 6 |
| `Unknown` → `Sat`（不一致） | 9 |

不一致 9 件はすべて `Unknown → Sat` で、原因は論理ではなく**タイムアウト粒度**である
（in-process は 1ms 等の実値、子は `-T:` が秒単位なので最大 1 秒使える）。実装するなら
ミリ秒の `-t:<ms>` を使い、硬い上限は親の kill に持たせること。

より重要なのは、子の出力に Z3 の `(error ...)` が 4 件現れたこと。`Z3_solver_to_string` は
モデル変換器の指示を混ぜて出力し、単体スクリプトとしては未宣言のシンボルを参照する:

```smt2
(model-del k!83)
(model-add a!80 () (_ BitVec 4) (mkbv k!83 k!84 k!85 k!86))
```

```text
(error "line 47 column 11: invalid function declaration reference, unknown function k!83")
```

さらに 58 件が `unsupported` / `pb2bv-model-converter` を出力していた。つまり
**`Display` 出力をそのまま子に渡す方式は不正**であり、実装するなら
`solver.get_assertions()` を `z3_sys::Z3_benchmark_to_smtlib_string` に通してモデル指示を
含まないスクリプトを生成する必要がある。

### 2.3 忠実性の検証範囲が足りていない

上記コーパスに現れた sort は整数/真偽/非線形算術が主で、BitVec が 4 件（上記エラー該当）。
**量化子・配列・文字列・未解釈 sort・refinement 符号化は 1 件も通っていない**。したがって
現時点のデータは「これらの往復が安全」を一切保証しない。A を実装する場合は、これらを含む
コーパスで in-process と子の結果一致をゲートすることが前提条件になる。

### 2.4 コスト

| 区分 | min | median | max |
|---|---:|---:|---:|
| 子プロセス | 11.523ms | 13.068ms | 5012.458ms |
| in-process | 0.119ms | 2.915ms | 5003.743ms |

std コーパス（59 モジュール / 2163 チェック、Z3 4.14.1、キャッシュ削除後）:

| モード | 結果 | 実時間 |
|---|---|---:|
| in-process | 59/59 | 5.664s |
| 強制サブプロセス | 59/59 | 28.626s |

**5.05x**。1 チェックあたり約 11ms のプロセス起動が下限として乗るため、常時有効化はできない。

### 2.5 A を実装する場合の必須設計

1. 既定は in-process。`z3_sys::Z3_get_version` が 4.14 未満のときだけ子プロセスへ切り替える（新しい環境はコストを払わない）。
2. SMT2 生成は `Display` ではなく `get_assertions()` + `Z3_benchmark_to_smtlib_string`。
3. 子の出力に `(error` が含まれたら結果は必ず `Unknown` に落とす（`Unsat` として採用しない）。矛盾検出は `Unsat` のみが引き金なので、この規則があれば劣化方向は安全側（見逃し）に閉じる。
4. タイムアウトは `-t:<ms>`、硬い上限は親の kill。
5. `z3` バイナリの探索順は `~/.mumei/toolchains/z3-*/bin/z3` → `PATH` → 見つからなければ in-process にフォールバックし、上限が効かないことを警告する（`src/setup.rs` のレイアウトは `toolchains/z3-<version>/bin/z3`）。

## 3. 候補 B（最低バージョン拒否）

`Z3_get_version` が 4.14 未満なら検証を実行せずエラーにする案。実装は数行だが、
§1 の 6 ジョブは distro の `libz3-dev` を入れているため、
**先に CI 側を新しい Z3 に移さないと自分の CI が止まる**。
distro libz3 で開発している利用者も一律に締め出す。単独では採らない。

## 4. 候補 C（推奨）

1. `verify` 実行時にリンク先 libz3 が 4.14 未満なら、spec-health の `--solver-timeout` が
   硬い上限として効かないことを一度だけ警告する（#527 の限界を利用者に見える形にする）。
   実装は `solver_timeout_is_hard()` / `linked_z3_version()` と `verify` 側の一回限りの stderr 警告。
2. §1 の 6 ジョブに `.github/actions/setup-z3` を挟み、上流プレビルド 4.14.1 に対して
   ビルド・実行させる（`Z3_SYS_Z3_HEADER` / `Z3_SYS_Z3_LIB_DIR` / `LD_LIBRARY_PATH`）。
   これにより CI 上のハング露出が消え、B を将来採る前提も整う。副作用として、これまで
   バージョンゲートでスキップされていた `hard_nonlinear_spec_validation_respects_timeout` が
   CI で初めて実際に実行される。
3. A は §2.5 の前提条件付きの逃げ道として本ドキュメントに残す。

C は挙動を変えないため、5.05x のコストも往復忠実性のリスクも負わない。

## 5. 未解決の論点

- distro libz3 を一級構成として支えるか（支えるなら A、支えないなら B へ進める）。
- A を実装する場合、量化子/配列/文字列/未解釈 sort を含む忠実性コーパスをどこに置くか。
- 4.8.12 のどのステップが interrupt も `rlimit` も参照しないのか（#527 の計測では
  `rlimit` 100k〜10M すべて打ち切れず、`rlimit=1` のみ停止）。原因特定は本調査の範囲外。

## 6. 計測の再現

```sh
# 子プロセス経路の硬タイムアウト（system Z3 4.8.12）
z3 -in -T:1 < <hanging-goal.smt2>

# 忠実性比較・std コーパス（Z3 4.14.1 を preload）
LD_PRELOAD="$HOME/.mumei/toolchains/z3-4.14.1/bin/libz3.so" \
cargo test -p mumei-core --lib spec_validation
```

プロトタイプは commit していない（`mumei-core/src/verification/spec_validation.rs` への
一時計測パッチとして実行）。本 PR にコード変更は含まない。
