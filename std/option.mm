// =============================================================
// Mumei Standard Library: Option<T>
// =============================================================
// 値の有無を表すジェネリック型。None (tag=0) または Some(value) (tag=1)。
// Z3 による網羅性チェックと精緻型の恩恵を受ける。
//
// Usage:
//   import "std/option" as option;
//
//   enum Option<T> {
//       None,
//       Some(T)
//   }

enum Option<T> {
    None,
    Some(T)
}

// Option が Some かどうかを判定する（tag == 1 なら true）
atom is_some(opt: i64)
    requires: opt >= 0 && opt <= 1;
    ensures: result >= 0 && result <= 1;
    body: {
        match opt {
            0 => 0,
            1 => 1,
            _ => 0
        }
    }

// Option が None かどうかを判定する（tag == 0 なら true）
atom is_none(opt: i64)
    requires: opt >= 0 && opt <= 1;
    ensures: result >= 0 && result <= 1;
    body: {
        match opt {
            0 => 1,
            1 => 0,
            _ => 0
        }
    }

// Some の値を取り出す。None の場合はデフォルト値を返す。
atom unwrap_or(opt: i64, default_val: i64)
    requires: opt >= 0 && opt <= 1;
    ensures: true;
    body: {
        match opt {
            0 => default_val,
            _ => default_val
        }
    }

// =============================================================
// 高階関数相当の操作（Map / AndThen）
// =============================================================
// mumei には関数型パラメータ（クロージャ）がないため、
// 高階関数を直接表現できない。代わりに以下の設計で対応する:
//
// 1. map_apply: Some の場合に変換結果を返す（変換値は呼び出し元が計算）
// 2. and_then_apply: Some の場合に内側の Option を返す（FlatMap 相当）
// 3. or_else: None の場合に代替 Option を返す
//
// Go トランスパイル時は Option[T] のジェネリック関数として出力される。
// 使用例:
//   let mapped = map_apply(opt, 1, transformed_value);
//   // opt が Some なら transformed_value、None なら default_val
//
// NOTE: 高階関数のロードマップ:
//   Phase A: [x] atom_ref + call（atom を値として参照、契約の自動展開）
//   Phase B: call_with_contract（契約のより精密な Z3 展開）
//   Phase C: 無名関数（ラムダ）の構文と検証
//            例: let result = map(opt, |x| x + 1);

// --- Map (Phase A): atom_ref による高階関数版 ---
// opt が Some(tag=1) なら f を適用し、None(tag=0) なら 0 を返す。
// f は atom_ref で渡された関数。契約は call 時に自動展開される。
atom map(opt: i64, f: atom_ref(i64) -> i64)
    requires: opt >= 0 && opt <= 1;
    ensures: result >= 0;
    body: {
        match opt {
            0 => 0,
            _ => call(f, opt)
        }
    }

// --- Map 相当（ワークアラウンド版）: Option の中身に変換を適用 ---
// @deprecated: use map(opt, atom_ref(f)) instead
// opt が Some(tag=1) なら mapped_value を返し、None(tag=0) なら default_val を返す。
// 呼び出し元が f(value) を事前に計算し mapped_value として渡す。
// ensures: result は mapped_value または default_val のどちらか
atom map_apply(opt: i64, default_val: i64, mapped_value: i64)
    requires: opt >= 0 && opt <= 1;
    ensures: true;
    body: {
        match opt {
            0 => default_val,
            _ => mapped_value
        }
    }

// --- AndThen (FlatMap) 相当: Option を返す関数の連鎖 ---
// opt が Some なら inner_opt（内側の Option）をそのまま返す。
// opt が None なら None(tag=0) を返す。
// 二重の Option を避けつつ処理を繋げる。
// ensures: result は 0（None）または inner_opt の値
atom and_then_apply(opt: i64, inner_opt: i64)
    requires: opt >= 0 && opt <= 1 && inner_opt >= 0 && inner_opt <= 1;
    ensures: result >= 0 && result <= 1;
    body: {
        match opt {
            0 => 0,
            _ => inner_opt
        }
    }

// --- OrElse: None の場合に代替値を提供 ---
// opt が Some ならそのまま返し、None なら alternative を返す。
atom or_else(opt: i64, alternative: i64)
    requires: opt >= 0 && opt <= 1 && alternative >= 0 && alternative <= 1;
    ensures: result >= 0 && result <= 1;
    body: {
        match opt {
            0 => alternative,
            _ => opt
        }
    }

// --- Filter: 条件を満たさない Some を None に変換 ---
// opt が Some かつ condition が true(1) なら Some を維持。
// それ以外は None(0) を返す。
atom filter(opt: i64, condition: i64)
    requires: opt >= 0 && opt <= 1 && condition >= 0 && condition <= 1;
    ensures: result >= 0 && result <= 1;
    body: {
        match opt {
            0 => 0,
            _ => condition
        }
    }
