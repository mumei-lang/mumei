// =============================================================
// std/core_predicates — core-seeded predicate helpers
// =============================================================
// 明示 body を持つ forge task から決定的に生成した検証対象。
// LLM 呼び出しなしで forge できる構造的契約のみを含む。
//
// Usage:
//   import "core_predicates" as core_predicates;

// --- safe_index_or_zero ---
// A valid index into a `len`-element array satisfies `idx < len`; the
// `<= len` bound admits the one-past-end index.
atom safe_index_or_zero(idx: i64, len: i64)
    requires: len >= 0;
    ensures: result >= 0 && (result == 0 || result < len);
    body: {
        if idx >= 0 { if idx < len { idx } else { 0 } } else { 0 }
    };

// --- is_nonzero_flag ---
atom is_nonzero_flag(value: i64)
    requires: true;
    ensures: result == 0 || result == 1;
    body: {
        if value != 0 { 1 } else { 0 }
    };

// --- preserve_safe_index ---
atom preserve_safe_index(idx: i64, len: i64)
    requires: idx >= 0 && len >= 1 && idx < len;
    ensures: result >= 0 && result < len;
    body: {
        idx
    };
