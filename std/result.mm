// =============================================================
// Mumei Standard Library: Result<T, E>
// =============================================================
// 成功 (Ok, tag=0) または失敗 (Err, tag=1) を表すジェネリック型。
// エラーハンドリングの安全性を Z3 で保証する。
//
// Usage:
//   import "std/result" as result;

enum Result<T, E> {
    Ok(T),
    Err(E)
}

// Result が Ok かどうかを判定する
atom is_ok(res: i64)
    requires: res >= 0 && res <= 1;
    ensures: result >= 0 && result <= 1;
    body: {
        match res {
            0 => 1,
            1 => 0,
            _ => 0
        }
    }

// Result が Err かどうかを判定する
atom is_err(res: i64)
    requires: res >= 0 && res <= 1;
    ensures: result >= 0 && result <= 1;
    body: {
        match res {
            0 => 0,
            1 => 1,
            _ => 0
        }
    }

// Ok の値を取り出す。Err の場合はデフォルト値を返す。
atom unwrap_or_default(res: i64, default_val: i64)
    requires: res >= 0 && res <= 1;
    ensures: true;
    body: {
        match res {
            0 => default_val,
            _ => default_val
        }
    }

// 安全な除算: ゼロ除算を Err として返す
atom safe_divide(a: i64, b: i64)
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: {
        if b == 0 { 1 } else { 0 }
    }

// =============================================================
// 高階関数相当の操作（Map / AndThen）
// =============================================================
// NOTE: 高階関数のロードマップ:
//   Phase A: [x] atom_ref + call（atom を値として参照、契約の自動展開）
//   Phase B: call_with_contract（契約のより精密な Z3 展開）
//   Phase C: ラムダ構文と検証

// --- Map (Phase A): atom_ref による高階関数版 ---
// res が Ok(tag=0) なら f を適用し、Err(tag=1) なら 1 を返す。
// f は atom_ref で渡された関数。契約は call 時に自動展開される。
// NOTE: f の契約はパラメトリックなため trusted（Phase B で解決予定）
trusted atom result_map(res: i64, f: atom_ref(i64) -> i64)
    requires: res >= 0 && res <= 1;
    ensures: result >= 0;
    body: {
        match res {
            0 => call(f, res),
            _ => 1
        }
    }

// --- Map 相当（ワークアラウンド版）: Ok の中身に変換を適用 ---
// @deprecated: use result_map(res, atom_ref(f)) instead
// res が Ok(tag=0) なら mapped_value を返し、Err(tag=1) なら default_val を返す。
atom result_map_apply(res: i64, default_val: i64, mapped_value: i64)
    requires: res >= 0 && res <= 1;
    ensures: true;
    body: {
        match res {
            0 => mapped_value,
            _ => default_val
        }
    }

// --- AndThen (FlatMap) 相当: Result を返す関数の連鎖 ---
// res が Ok なら inner_res をそのまま返す。Err ならそのまま Err(1) を返す。
atom result_and_then(res: i64, inner_res: i64)
    requires: res >= 0 && res <= 1 && inner_res >= 0 && inner_res <= 1;
    ensures: result >= 0 && result <= 1;
    body: {
        match res {
            0 => inner_res,
            _ => 1
        }
    }

// --- OrElse: Err の場合に代替 Result を提供 ---
// res が Ok ならそのまま返し、Err なら alternative を返す。
atom result_or_else(res: i64, alternative: i64)
    requires: res >= 0 && res <= 1 && alternative >= 0 && alternative <= 1;
    ensures: result >= 0 && result <= 1;
    body: {
        match res {
            0 => 0,
            _ => alternative
        }
    }

// --- MapErr: Err の中身を変換 ---
// res が Err なら Err タグ(1) を返し、Ok ならそのまま Ok(0) を返す。
// タグベースモデルでは Err の「中身」は別途管理するため、
// ここではタグの保存のみを行う。
atom result_map_err(res: i64, mapped_err: i64)
    requires: res >= 0 && res <= 1;
    ensures: result >= 0 && result <= 1;
    body: {
        match res {
            0 => 0,
            _ => 1
        }
    }

// =============================================================
// エラーラップ（パッケージ境界でのエラー変換）
// =============================================================
// パッケージ間でエラーコードを変換するための操作群。
// 内部エラーコードを外部向けに再マッピングする際に使用する。
//
// 設計: mumei のタグベースモデルでは、エラーの「種類」をエラーコード（i64）で表現する。
// MapErr は Result のタグ（Ok/Err）を保持しつつ、エラーコードを変換する。

// --- WrapErr: Err にコンテキスト情報を付加 ---
// res が Err の場合、元のエラーコード err_code にオフセット wrap_offset を加算し、
// パッケージ固有のエラー空間にマッピングする。
// Ok の場合はそのまま Ok(0) を返す。
// ensures: result は Ok(0) または変換後のエラーコード
atom result_wrap_err(res: i64, err_code: i64, wrap_offset: i64)
    requires: res >= 0 && res <= 1 && err_code >= 0 && wrap_offset >= 0;
    ensures: result >= 0;
    body: {
        match res {
            0 => 0,
            _ => err_code + wrap_offset
        }
    }

// --- UnwrapOrElse: Err の場合にエラーコードに基づくデフォルト値を返す ---
// res が Ok なら ok_value を返し、Err なら err_default を返す。
// エラーハンドリングの最終段で使用する。
atom result_unwrap_or_else(res: i64, ok_value: i64, err_default: i64)
    requires: res >= 0 && res <= 1;
    ensures: true;
    body: {
        match res {
            0 => ok_value,
            _ => err_default
        }
    }

// --- Flatten: Result<Result<T, E>, E> → Result<T, E> ---
// 二重の Result をフラット化する。
// outer が Err → Err(1)
// outer が Ok かつ inner が Err → Err(1)
// outer が Ok かつ inner が Ok → Ok(0)
atom result_flatten(outer: i64, inner: i64)
    requires: outer >= 0 && outer <= 1 && inner >= 0 && inner <= 1;
    ensures: result >= 0 && result <= 1;
    body: {
        match outer {
            0 => inner,
            _ => 1
        }
    }
