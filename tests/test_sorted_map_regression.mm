// =============================================================
// tests/test_sorted_map_regression.mm — M2 trusted-atom reduction regression
// =============================================================
// `std/container/sorted_map.mm::sorted_map_insert` から `trusted` を除去した後、
// 末尾挿入 (append-at-end) の quantified array-store 不変量が Z3 decidable
// fragment で閉じることを回帰ゲート化する。bounded forall は明示 index range
// (`0..map_len`) へ lowering される。append / remove-tail / no-op removal の
// 3 ケースを固定し、Z3 が `unknown` を返して trusted へ後退していないことを保証する。
// =============================================================

// 末尾挿入 (append): keys[map_len] = key の store 後、区間 [0, map_len] が
// ソート済みであることを直接証明する。従来 trusted だった中核 obligation。
atom regression_sorted_map_append_store(map_len: i64, map_cap: i64, key: i64)
requires: map_len >= 0
    && map_cap > 0
    && map_len < map_cap
    && len(keys) >= map_cap
    && forall(i, 0, map_len, keys[i] <= key)
    && forall(i, 0, map_len - 1, keys[i] <= keys[i + 1]);
ensures: result == map_len + 1
    && forall(i, 0, result - 1, keys[i] <= keys[i + 1]);
body: {
    keys[map_len] = key;
    map_len + 1
};

// 末尾削除 (remove-tail): 長さを 1 減らしても残りのソート順は保存される。
atom regression_sorted_map_remove_tail(map_len: i64)
requires: map_len > 0
    && len(keys) >= map_len
    && forall(i, 0, map_len - 1, keys[i] <= keys[i + 1]);
ensures: result == map_len - 1
    && forall(i, 0, result - 1, keys[i] <= keys[i + 1]);
body: {
    map_len - 1
};

// 何もしない削除 (no-op removal): 長さ不変でソート順も不変。
atom regression_sorted_map_remove_noop(map_len: i64)
requires: map_len > 0
    && len(keys) >= map_len
    && forall(i, 0, map_len - 1, keys[i] <= keys[i + 1]);
ensures: result == map_len
    && forall(i, 0, result - 1, keys[i] <= keys[i + 1]);
body: {
    map_len
};
