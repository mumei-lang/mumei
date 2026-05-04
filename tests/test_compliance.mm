// RegTech Compliance E2E Tests

type RiskScore = i64 where v >= 0 && v <= 100;
type TransactionAmount = i64 where v >= 0;

enum CustomerType {
    Individual,
    Corporate,
    Government,
    PEP
}

enum RiskLevel {
    Low,
    Medium,
    High,
    Critical
}

// リスク分類の網羅性テスト
atom test_classify_all_types(ct: i64)
    requires: ct >= 0 && ct <= 3;
    ensures: result >= 0 && result <= 3;
    body: {
        match ct {
            0 => 0,
            1 => 1,
            2 => 0,
            3 => 3,
            _ => 2
        }
    }

// 限度額の正値性テスト
atom test_limit_positive(rl: i64)
    requires: rl >= 0 && rl <= 3;
    ensures: result > 0;
    body: {
        match rl {
            0 => 1000000,
            1 => 500000,
            2 => 100000,
            3 => 10000,
            _ => 10000
        }
    }

// forall: 全取引が限度額以下
atom test_all_compliant(n: i64, limit: i64)
    requires: n >= 0 && limit > 0 && forall(i, 0, n, arr[i] >= 0 && arr[i] <= limit);
    ensures: result == 1;
    body: 1

// ガード付き match: 承認レベル
atom test_approval_level(amount: TransactionAmount)
    requires: amount >= 0;
    ensures: result >= 0 && result <= 3;
    body: {
        match amount {
            a if a <= 10000 => 0,
            a if a <= 100000 => 1,
            a if a <= 1000000 => 2,
            _ => 3
        }
    }
