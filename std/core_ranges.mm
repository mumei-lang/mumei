// =============================================================
// std/core_ranges — core-seeded interval range predicates
// =============================================================
// 明示 body を持つ forge task から決定的に生成した検証対象。
// LLM 呼び出しなしで forge できる構造的契約のみを含む。
//
// Usage:
//   import "core_ranges" as core_ranges;

// --- ranges_disjoint ---
atom ranges_disjoint(a_lo: i64, a_hi: i64, b_lo: i64, b_hi: i64)
    requires: a_lo <= a_hi && b_lo <= b_hi;
    ensures: result == 0 || result == 1;
    body: {
        if a_hi < b_lo { 1 } else { if b_hi < a_lo { 1 } else { 0 } }
    };

// --- ranges_overlap ---
atom ranges_overlap(a_lo: i64, a_hi: i64, b_lo: i64, b_hi: i64)
    requires: a_lo <= a_hi && b_lo <= b_hi;
    ensures: result == 0 || result == 1;
    body: {
        if a_hi < b_lo { 0 } else { if b_hi < a_lo { 0 } else { 1 } }
    };

// --- range_width_nonneg ---
atom range_width_nonneg(lo: i64, hi: i64)
    requires: lo >= 0 && lo <= hi;
    ensures: result >= 0 && result <= hi;
    body: {
        hi - lo
    };

// --- point_before_range ---
atom point_before_range(lo: i64, hi: i64, p: i64)
    requires: lo <= hi;
    ensures: result == 0 || result == 1;
    body: {
        if p < lo { 1 } else { 0 }
    };
