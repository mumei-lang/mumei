// =============================================================
// examples/libc_demo.mm — Verified C Library Demo
// =============================================================
// std/libc の検証済みラッパーを使用したデモ。
// メモリ確保 → 使用 → 解放のパイプラインと、
// Z3 がリジェクトする不安全な呼び出しの例を示す。
//
// Safe calls:   mumei verify examples/libc_demo.mm   → should pass
// Unsafe calls: コメント内の unsafe 例は検証に失敗する
//
// Usage:
//   mumei verify examples/libc_demo.mm
//   mumei build examples/libc_demo.mm --emit c-header

import "std/libc" as libc;

// =============================================================
// Safe examples — Z3 がすべて検証をパスする
// =============================================================

// 1. 基本的なメモリ確保
atom demo_malloc_basic()
    requires: true;
    ensures: result >= -1;
    body: libc::safe_malloc(1024);

// 2. ゼロ初期化メモリ確保（calloc）
atom demo_calloc_array()
    requires: true;
    ensures: result >= -1;
    body: libc::safe_calloc(100, 8);

// 3. 安全なメモリコピー
atom demo_safe_copy(buf_size: i64, n: i64)
    requires: buf_size > 0 && n >= 0 && n <= buf_size;
    ensures: result >= 0;
    body: libc::safe_memcpy(buf_size, buf_size, n);

// 4. 安全なメモリ移動（オーバーラップ許容）
atom demo_safe_move(buf_size: i64, n: i64)
    requires: buf_size > 0 && n >= 0 && n <= buf_size;
    ensures: result >= 0;
    body: libc::safe_memmove(buf_size, buf_size, n);

// 5. バッファのゼロクリア
atom demo_zero_fill(buf_size: i64)
    requires: buf_size > 0;
    ensures: result >= 0;
    body: libc::safe_memset(buf_size, 0, buf_size);

// 6. 文字列長の安全な取得
atom demo_strlen(buf_size: i64)
    requires: buf_size > 0;
    ensures: result >= 0 && result < buf_size;
    body: libc::safe_strlen(buf_size);

// 7. メモリ再確保（realloc）
atom demo_realloc_grow(ptr: i64, old_size: i64, new_size: i64)
    requires: ptr >= 0 && old_size >= 0 && new_size > 0;
    ensures: result >= -1;
    body: libc::safe_realloc(ptr, old_size, new_size);

// 8. メモリ確保 → 使用 → 解放パイプライン
//    malloc でバッファ確保 → memset でゼロクリア → free で解放
//    malloc は -1（確保失敗）を返す可能性があるため、ガードが必要
atom demo_alloc_use_free_pipeline()
    requires: true;
    ensures: result >= -1;
    body: {
        // malloc でバッファ確保（result >= -1）
        let ptr = libc::safe_malloc(256);
        // 確保成功時のみ memset → free を実行
        if ptr >= 0 then {
            let cleared = libc::safe_memset(256, 0, 256);
            libc::safe_free(ptr)
        } else ptr
    };

// 9. 安全な snprintf
atom demo_snprintf(buf_size: i64)
    requires: buf_size > 0;
    ensures: result >= 0;
    body: libc::safe_snprintf(buf_size, buf_size);

// =============================================================
// Unsafe examples (commented out) — Z3 がリジェクトする
// =============================================================
// これらのコメントを解除すると、mumei verify が失敗する。
// 各例は requires 句に違反する呼び出しを含む。

// 例 1: バッファオーバーフロー（dst_size < n）
// atom unsafe_memcpy_overflow()
//     requires: true;
//     ensures: result >= 0;
//     body: libc::safe_memcpy(10, 100, 50);
//     // Error: dst_size(10) < n(50) — precondition_violated

// 例 2: ゼロサイズの malloc
// atom unsafe_malloc_zero()
//     requires: true;
//     ensures: result >= -1;
//     body: libc::safe_malloc(0);
//     // Error: size(0) not > 0 — precondition_violated

// 例 3: 負のポインタを free
// atom unsafe_free_negative()
//     requires: true;
//     ensures: result >= 0;
//     body: libc::safe_free(-1);
//     // Error: ptr(-1) not >= 0 — precondition_violated

// 例 4: memset の値がバイト範囲外
// atom unsafe_memset_bad_value()
//     requires: true;
//     ensures: result >= 0;
//     body: libc::safe_memset(64, 256, 32);
//     // Error: value(256) not <= 255 — precondition_violated

// 例 5: snprintf のバッファオーバーフロー
// atom unsafe_snprintf_overflow()
//     requires: true;
//     ensures: result >= 0;
//     body: libc::safe_snprintf(64, 128);
//     // Error: n(128) > buf_size(64) — precondition_violated
