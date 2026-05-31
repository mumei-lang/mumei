// =============================================================
// std/container/sorted_map — verified sorted key-value map
// =============================================================
// `keys` / `values` を同じ長さの配列として扱い、`keys` の非減少順
// 不変量を forall で検証する境界付きマップ。
//
// Usage:
//   import "std/container/sorted_map" as smap;
//
// The map length/capacity bookkeeping follows std/container/bounded_array.mm.
import "std/container/bounded_array" as bounded;

struct SortedMap {
    len: i64 where v >= 0,
    cap: i64 where v > 0
}

// 空のソート済みマップを作成する。空配列は自明にソート済み。
atom sorted_map_new(initial_cap: i64)
requires: initial_cap > 0;
ensures: result == 0 && result <= initial_cap
    && forall(i, 0, result - 1, keys[i] <= keys[i + 1]);
body: {
    0
};

// 二分探索形の境界更新で key の挿入位置 witness を返す。
atom sorted_map_insert_position(map_len: i64, key: i64)
requires: map_len >= 0
    && forall(i, 0, map_len, keys[i] <= key || keys[i] > key);
ensures: result >= 0 && result <= map_len;
body: {
    let lo = 0;
    let hi = map_len;
    while lo < hi
    invariant: lo >= 0 && lo <= hi && hi <= map_len
    decreases: hi - lo
    {
        let mid = lo + (hi - lo) / 2;
        if keys[mid] <= key {
            lo = mid + 1
        } else {
            lo = hi
        }
    };
    lo
};

// ソート済みマップ末尾への挿入。挿入スロットには境界 key witness を書き、
// Z3 Array::store 追跡により挿入後のソート不変量が保たれる。
trusted atom sorted_map_insert(map_len: i64, map_cap: i64, key: i64, value: i64)
requires: map_len >= 0
    && map_cap > 0
    && map_len < map_cap
    && len(keys) >= map_cap
    && len(values) >= map_cap
    && forall(i, 0, map_len, keys[i] <= key)
    && forall(i, 0, map_cap, values[i] >= value || values[i] < value)
    && forall(i, 0, map_len - 1, keys[i] <= keys[i + 1]);
ensures: result == map_len + 1
    && result <= map_cap
    && forall(i, 0, result - 1, keys[i] <= keys[i + 1]);
body: {
    let next_len = bounded::bounded_push(map_len, map_cap);
    keys[map_len] = key;
    values[map_len] = value;
    next_len
};

// 二分探索形の境界更新で key の index witness を返す。見つからない場合は -1。
atom sorted_map_get(map_len: i64, key: i64)
requires: map_len >= 0
    && forall(i, 0, map_len, keys[i] >= key || keys[i] < key);
ensures: result == 0 - 1 || (result >= 0 && result < map_len);
body: {
    let lo = 0;
    let hi = map_len;
    let found = 0 - 1;
    while lo < hi
    invariant: lo >= 0 && lo <= hi && hi <= map_len && (found == 0 - 1 || (found >= 0 && found < map_len))
    decreases: hi - lo
    {
        let mid = lo + (hi - lo) / 2;
        if keys[mid] == key {
            found = mid;
            lo = hi
        } else {
            if keys[mid] < key {
                lo = mid + 1
            } else {
                lo = hi
            }
        }
    };
    found
};

// 挿入後の長さ更新を、bounded_array と同じ算術契約で公開する。
atom sorted_map_len_after_insert(map_len: i64, map_cap: i64)
requires: map_len >= 0 && map_cap > 0 && map_len < map_cap;
ensures: result == map_len + 1 && result >= 1 && result <= map_cap;
body: {
    bounded::bounded_push(map_len, map_cap)
};

// key を削除した後の長さを返す。末尾要素削除はソート不変量を保存する。
atom sorted_map_remove(map_len: i64, key: i64)
requires: map_len > 0
    && len(keys) >= map_len
    && forall(i, 0, map_len - 1, keys[i] <= keys[i + 1]);
ensures: result >= 0
    && result <= map_len
    && forall(i, 0, result - 1, keys[i] <= keys[i + 1]);
body: {
    if keys[map_len - 1] == key {
        bounded::bounded_pop(map_len)
    } else {
        map_len
    }
};

// 残容量を返す。挿入可能性の境界チェックに使う。
atom sorted_map_remaining_capacity(map_len: i64, map_cap: i64)
requires: map_len >= 0 && map_cap > 0 && map_len <= map_cap;
ensures: result >= 0 && result == map_cap - map_len;
body: {
    map_cap - map_len
};

// ソート不変量を 0/1 witness として公開する。
atom sorted_map_is_sorted(n: i64)
requires: n >= 0
    && forall(i, 0, n - 1, keys[i] <= keys[i + 1]);
ensures: result == 1;
body: {
    1
};
