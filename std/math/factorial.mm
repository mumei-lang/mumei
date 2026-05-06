// =============================================================
// std/math/factorial — verified factorial
// =============================================================
// 階乗計算に使う小さな Z3 検証済みヘルパー。
// 実際の再帰本体ではなく、各ステップと入力範囲の契約を提供する。

atom factorial_step(acc: i64, n: i64)
    requires: acc >= 1 && n >= 1 && n <= 20 && acc <= 1000000;
    ensures: result == acc * n && result >= 1;
    body: {
        acc * n
    };

atom factorial_in_range(n: i64)
    requires: n >= 0;
    ensures: result >= 1;
    body: {
        1
    };
