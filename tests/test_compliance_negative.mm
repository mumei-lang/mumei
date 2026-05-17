// ❌ LLM が生成したバグ入りコード
// PEP (Politically Exposed Person) カテゴリが match から漏れている
// → 網羅性欠如エラーが期待される

enum CustomerType {
    Individual,
    Corporate,
    Government,
    PEP
}

atom buggy_classify_risk(customer_type: CustomerType)
    requires: true;
    ensures: result >= 0 && result <= 3;
    body: {
        match customer_type {
            Individual => 0,
            Corporate => 1,
            Government => 0
        }
    }

// 負テスト: PEP カテゴリが match から漏れているケース
atom test_missing_pep_arm(customer_type: CustomerType)
    requires: true;
    ensures: result >= 0 && result <= 3;
    body: {
        match customer_type {
            Individual => 0,
            Corporate => 1,
            Government => 0
            // PEP が意図的に省略されている
        }
    }
