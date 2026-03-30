// =============================================================
// Mumei Standard Library: Verified Contracts
// =============================================================
// 汎用の精緻型・検証済みバリデータ・安全な算術操作のカタログ。
// AI エージェントが新規コード生成前に再利用可能な検証済み部品を
// 発見するための標準ライブラリモジュール。
//
// Usage: import "std/contracts" as contracts;

// =============================================================
// Refinement Types（精緻型）
// =============================================================

type Port = i64 where v >= 1 && v <= 65535;
type Percentage = i64 where v >= 0 && v <= 100;
type PositiveAmount = i64 where v > 0;
type NonNegative = i64 where v >= 0;
type Byte = i64 where v >= 0 && v <= 255;
type HttpStatus = i64 where v >= 100 && v < 600;
type ExitCode = i64 where v >= 0 && v <= 255;

// =============================================================
// Atoms: Range Validation（範囲検証）
// =============================================================

// 値が指定範囲内にあるかチェック（0=false, 1=true）
atom is_within_range(val: i64, min_val: i64, max_val: i64)
    requires: min_val <= max_val;
    ensures: result >= 0 && result <= 1;
    body: {
        if val >= min_val && val <= max_val { 1 } else { 0 }
    };

// 値を指定範囲にクランプ
atom clamp(val: i64, min_val: i64, max_val: i64)
    requires: min_val <= max_val;
    ensures: result >= min_val && result <= max_val;
    body: {
        if val < min_val { min_val }
        else { if val > max_val { max_val } else { val } }
    };

// 絶対値を返す
atom abs_val(x: i64)
    requires: true;
    ensures: result >= 0;
    body: {
        if x >= 0 { x } else { 0 - x }
    };

// 2 つの値の最大値を返す
atom max_of(a: i64, b: i64)
    requires: true;
    ensures: result >= a && result >= b;
    body: {
        if a >= b { a } else { b }
    };

// 2 つの値の最小値を返す
atom min_of(a: i64, b: i64)
    requires: true;
    ensures: result <= a && result <= b;
    body: {
        if a <= b { a } else { b }
    };

// =============================================================
// Atoms: Domain Validation（ドメイン検証）
// =============================================================

// ポート番号が有効かチェック（1-65535）
atom is_valid_port(port: i64)
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: {
        if port >= 1 && port <= 65535 { 1 } else { 0 }
    };

// HTTP ステータスコードが有効かチェック（100-599）
atom is_valid_http_status(status: i64)
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: {
        if status >= 100 && status < 600 { 1 } else { 0 }
    };

// =============================================================
// Atoms: Safe Arithmetic（安全な算術）
// =============================================================

// ゼロ除算防止付き除算
atom safe_divide(a: i64, b: i64)
    requires: b != 0;
    ensures: true;
    body: {
        a / b
    };

// ゼロ除算防止付き剰余演算
atom safe_modulo(a: i64, b: i64)
    requires: b > 0;
    ensures: result >= 0;
    body: {
        let r = a - (a / b) * b;
        if r >= 0 { r } else { r + b }
    };
