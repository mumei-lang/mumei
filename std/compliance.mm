// =============================================================
// Mumei Standard Library: RegTech Compliance Protocol
// =============================================================
// KYC/AML コンプライアンスチェック: 顧客分類の網羅性と
// 取引限度額の遵守を Z3 でコンパイル時に保証する。
//
// 検証される性質:
//   - match 網羅性: 全顧客タイプ（Individual, Corporate, Government, PEP）が
//     リスク分類で漏れなくカバーされている
//   - forall 量化子: 全取引が規制限度額を遵守している
//   - 精緻型: リスクスコアと取引額の値域制約
//
// Usage:
//   import "std/compliance" as compliance;

// --- 精緻型: 値域制約 ---
type RiskScore = i64 where v >= 0 && v <= 100;
type TransactionAmount = i64 where v >= 0;

// --- Enum: 顧客タイプ ---
// 0 = Individual, 1 = Corporate, 2 = Government, 3 = PEP
enum CustomerType {
    Individual,
    Corporate,
    Government,
    PEP
}

// --- Enum: リスクレベル ---
// 0 = Low, 1 = Medium, 2 = High, 3 = Critical
enum RiskLevel {
    Low,
    Medium,
    High,
    Critical
}

// --- 基本 atom: 顧客タイプからリスクレベルへの分類 ---
// match 網羅性により、全顧客タイプが必ずリスクレベルにマッピングされる
atom classify_risk(customer_type: i64)
    requires: customer_type >= 0 && customer_type <= 3;
    ensures: result >= 0 && result <= 3;
    body: {
        match customer_type {
            0 => 0,
            1 => 1,
            2 => 0,
            3 => 3,
            _ => 2
        }
    }

// --- 基本 atom: リスクレベルに基づく取引限度額の決定 ---
// match 網羅性により、全リスクレベルに限度額が設定される
atom get_transaction_limit(risk_level: i64)
    requires: risk_level >= 0 && risk_level <= 3;
    ensures: result > 0;
    body: {
        match risk_level {
            0 => 1000000,
            1 => 500000,
            2 => 100000,
            3 => 10000,
            _ => 10000
        }
    }

// --- 基本 atom: 単一取引のコンプライアンスチェック ---
// 取引額が顧客のリスクレベルに基づく限度額以下であることを検証
atom check_transaction(customer_type: i64, amount: TransactionAmount)
    requires: customer_type >= 0 && customer_type <= 3 && amount >= 0;
    ensures: result >= 0 && result <= 1;
    body: {
        let risk = classify_risk(customer_type);
        let limit = get_transaction_limit(risk);
        if amount <= limit { 1 } else { 0 }
    }

// --- 合成 atom: 全取引の限度額遵守チェック ---
// forall 量化子で配列内の全取引が限度額以下であることを保証
atom verify_all_transactions_compliant(n: i64, limit: i64)
    requires: n >= 0 && limit > 0 && forall(i, 0, n, arr[i] >= 0 && arr[i] <= limit);
    ensures: result == 1;
    body: {
        1
    }

// --- 合成 atom: 顧客リスクスコアの閾値チェック ---
// forall 量化子で全顧客のリスクスコアが閾値以下であることを保証
atom verify_all_risk_scores_within_threshold(n: i64, threshold: RiskScore)
    requires: n >= 0 && threshold >= 0 && threshold <= 100 && forall(i, 0, n, arr[i] >= 0 && arr[i] <= threshold);
    ensures: result == 1;
    body: {
        1
    }

// --- 合成 atom: KYC 完全性チェック ---
// 顧客タイプの分類 + 取引チェックを合成
// match 網羅性 + forall 量化子の両方を活用
atom full_kyc_check(customer_type: i64, amount: TransactionAmount, n_history: i64, limit: i64)
    requires: customer_type >= 0 && customer_type <= 3 && amount >= 0 && n_history >= 0 && limit > 0 && forall(i, 0, n_history, arr[i] >= 0 && arr[i] <= limit);
    ensures: result >= 0 && result <= 1;
    body: {
        let risk = classify_risk(customer_type);
        let tx_limit = get_transaction_limit(risk);
        if amount <= tx_limit { 1 } else { 0 }
    }

// --- ガード付き match: 取引額に基づく承認レベル ---
// match + guards で取引額の範囲に応じた承認レベルを決定
atom approval_level(amount: TransactionAmount)
    requires: amount >= 0;
    ensures: result >= 0 && result <= 3;
    body: {
        match amount {
            a if a <= 10000 => 0,
            a if a <= 100000 => 1,
            a if a <= 1000000 => 2,
            _ => 3
        }
    };
