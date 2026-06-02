// =============================================================
// std/math/factorial — verified factorial
// =============================================================
// 階乗計算の検証可能な実装。
// n <= 20 の範囲でのみ正しい計算。n > 20 の場合、オーバーフローするため。
// 各 atom は要求事項と保証事項を Z3 にて完全に検証する。

import "std/core" as core;
import "std/math/safe_mul" as safe_mul;

// 階乗の一ステップを行う関数
atom factorial_step(acc: i64, n: i64) -> i64
    requires: acc >= 1 && n >= 1 && n <= 20 && acc <= 1000000;
    ensures: result == acc * n && result >= 1;
    body: {
        safe_mul::safe_mul(acc, n)
    };

// 範囲内で階乗が安全に計算可能かを判定
atom factorial_in_range(n: i64) -> i64
    requires: true;
    ensures: result == 0 || result == 1;
    body: {
        if n >= 0 && n <= 20 { 1 } else { 0 }
    };
