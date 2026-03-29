// =============================================================
// Verified Microservice: Role-Based Access Control (RBAC)
// =============================================================
// capability security パターンによる RBAC の検証。
// ユーザーのロールレベルとリソースのアクセスレベルを
// コンパイル時に Z3 が検証し、不正アクセスを防止する。
//
// ロールレベル:
//   0 = Guest, 1 = User, 2 = Admin, 3 = SuperAdmin
// リソースレベル:
//   0 = Public, 1 = Internal, 2 = Confidential, 3 = Restricted
//
// Usage:
//   mumei verify examples/verified_microservice/rbac.mm

// --- データアクセスエフェクト ---
// user_role >= resource_level の場合のみアクセスを許可
effect SafeDataAccess(user_role: i64, resource_level: i64)
    where user_role >= 0 && user_role <= 3
       && resource_level >= 0 && resource_level <= 3
       && user_role >= resource_level;

// --- アクセス権チェック ---
// ロールがリソースレベル以上であれば 1 を返す
atom check_access(user_role: i64, resource_level: i64)
    requires: user_role >= 0 && user_role <= 3
           && resource_level >= 0 && resource_level <= 3;
    ensures: result >= 0 && result <= 1;
    body: {
        if user_role >= resource_level { 1 } else { 0 }
    };

// --- 安全なデータ読み取り ---
// Admin (role >= 2) のみ Confidential (level 2) データを読める
atom admin_read_confidential(user_role: i64)
    effects: [SafeDataAccess(user_role, 2)]
    requires: user_role >= 2 && user_role <= 3;
    ensures: result >= 0;
    body: {
        perform SafeDataAccess.access(user_role, 2);
        1
    };

// --- 公開データ読み取り ---
// 全ロール (role >= 0) が Public (level 0) データを読める
atom read_public_data(user_role: i64)
    effects: [SafeDataAccess(user_role, 0)]
    requires: user_role >= 0 && user_role <= 3;
    ensures: result >= 0;
    body: {
        perform SafeDataAccess.access(user_role, 0);
        1
    };

// --- SuperAdmin 限定操作 ---
// SuperAdmin (role == 3) のみ Restricted (level 3) を操作可能
atom superadmin_restricted_op(user_role: i64)
    effects: [SafeDataAccess(user_role, 3)]
    requires: user_role == 3;
    ensures: result >= 0;
    body: {
        perform SafeDataAccess.access(user_role, 3);
        1
    };
