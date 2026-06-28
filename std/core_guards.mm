// =============================================================
// std/core_guards — core-seeded defensive guard predicates
// =============================================================
// 明示 body を持つ forge task から決定的に生成した検証対象。
// LLM 呼び出しなしで forge できる構造的契約のみを含む。
//
// Usage:
//   import "core_guards" as core_guards;

// --- is_in_bounds ---
atom is_in_bounds(val: i64, lo: i64, hi: i64)
    requires: lo <= hi;
    ensures: result == 0 || result == 1;
    body: {
        if val >= lo { if val <= hi { 1 } else { 0 } } else { 0 }
    };

// --- safe_abs_diff ---
atom safe_abs_diff(a: i64, b: i64)
    requires: a >= 0 && b >= 0;
    ensures: result >= 0;
    body: {
        if a >= b { a - b } else { b - a }
    };

// --- clamp_to_positive ---
atom clamp_to_positive(x: i64)
    requires: true;
    ensures: result >= 1;
    body: {
        if x >= 1 { x } else { 1 }
    };

// --- both_positive ---
atom both_positive(a: i64, b: i64)
    requires: true;
    ensures: result == 0 || result == 1;
    body: {
        if a >= 1 { if b >= 1 { 1 } else { 0 } } else { 0 }
    };
