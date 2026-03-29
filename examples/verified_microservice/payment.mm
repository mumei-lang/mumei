// =============================================================
// Verified Microservice: Payment Logic
// =============================================================
// 支払い計算ロジックを形式的に検証する。
// 各 atom の requires/ensures 契約を Z3 が証明し、
// オーバーフローや不正な値が発生しないことを保証する。
//
// Usage:
//   mumei verify examples/verified_microservice/payment.mm
//   mumei build examples/verified_microservice/payment.mm --emit c-header

// --- 小計計算 ---
// price * quantity のオーバーフローを境界で防止
atom calc_subtotal(price: i64, quantity: i64)
    requires: price >= 0 && quantity >= 0 && quantity <= 10000 && price <= 1000000;
    ensures: result == price * quantity && result >= 0;
    body: { price * quantity };

// --- 税額計算 ---
// tax_rate_pct は 0〜100 のパーセンテージ
atom calc_tax(amount: i64, tax_rate_pct: i64)
    requires: amount >= 0 && amount <= 10000000000 && tax_rate_pct >= 0 && tax_rate_pct <= 100;
    ensures: result >= 0 && result == amount * tax_rate_pct / 100;
    body: { amount * tax_rate_pct / 100 };

// --- 合計計算 ---
// subtotal + tax を一括計算
atom calc_total(price: i64, quantity: i64, tax_rate_pct: i64)
    requires: price >= 0 && quantity >= 0 && quantity <= 10000
           && price <= 1000000 && tax_rate_pct >= 0 && tax_rate_pct <= 100;
    ensures: result >= 0;
    body: {
        let subtotal = price * quantity;
        subtotal + subtotal * tax_rate_pct / 100
    };
