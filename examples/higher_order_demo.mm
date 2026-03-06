// =============================================================
// Higher-Order Functions Demo (Phase A: atom_ref + call)
// =============================================================
// atom_ref で関数を第一級の値として参照し、call で呼び出す。
// 参照先の atom の契約（requires/ensures）は call 時に自動展開される。
//
// Usage:
//   mumei verify examples/higher_order_demo.mm
//   mumei build examples/higher_order_demo.mm

// --- 基本的な atom ---
atom increment(x: i64)
    requires: x >= 0;
    ensures: result == x + 1;
    body: x + 1;

atom double(x: i64)
    requires: x >= 0;
    ensures: result == x * 2;
    body: x * 2;

atom add(a: i64, b: i64)
    requires: a >= 0 && b >= 0;
    ensures: result == a + b;
    body: a + b;

// --- atom_ref + call の基本的な使用 ---
// atom_ref(increment) で increment を関数参照として取得し、call で呼び出す。
atom apply(x: i64, f: atom_ref(i64) -> i64)
    requires: x >= 0;
    ensures: result >= 0;
    body: call(f, x);

// --- 高階関数: 関数を2回適用する ---
atom apply_twice(x: i64, f: atom_ref(i64) -> i64)
    requires: x >= 0;
    ensures: result >= 0;
    body: {
        let first = call(f, x);
        call(f, first)
    }

// --- 高階関数: 二項関数を畳み込みに使用 ---
atom fold_two(a: i64, b: i64, f: atom_ref(i64, i64) -> i64)
    requires: a >= 0 && b >= 0;
    ensures: result >= 0;
    body: call(f, a, b);

// --- 使用例を示す atom ---
// apply(5, atom_ref(increment)) → 6
atom demo_apply()
    requires: true;
    ensures: result >= 0;
    body: apply(5, atom_ref(increment));

// apply_twice(5, atom_ref(increment)) → 7
atom demo_apply_twice()
    requires: true;
    ensures: result >= 0;
    body: apply_twice(5, atom_ref(increment));

// fold_two(3, 4, atom_ref(add)) → 7
atom demo_fold()
    requires: true;
    ensures: result >= 0;
    body: fold_two(3, 4, atom_ref(add));
