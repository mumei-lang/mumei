// =============================================================
// tests/test_sorted_map.mm — SortedMap integration tests
// =============================================================

import "std/container/sorted_map" as smap;

atom test_sorted_map_insert(map_len: i64, map_cap: i64, key: i64, value: i64)
requires: map_len >= 0
    && map_cap > 0
    && map_len < map_cap
    && len(keys) >= map_cap
    && len(values) >= map_cap
    && forall(i, 0, map_len, keys[i] <= key)
    && forall(i, 0, map_cap, values[i] >= value || values[i] < value)
    && forall(i, 0, map_len - 1, keys[i] <= keys[i + 1]);
ensures: result == map_len + 1
    && forall(i, 0, result - 1, keys[i] <= keys[i + 1]);
body: {
    smap::sorted_map_insert(map_len, map_cap, key, value)
};

atom test_sorted_map_get(map_len: i64, key: i64)
requires: map_len >= 0
    && forall(i, 0, map_len, keys[i] >= key || keys[i] < key);
ensures: result == 0 - 1 || (result >= 0 && result < map_len);
body: {
    smap::sorted_map_get(map_len, key)
};

atom test_sorted_map_remove(map_len: i64, key: i64)
requires: map_len > 0
    && len(keys) >= map_len
    && forall(i, 0, map_len - 1, keys[i] <= keys[i + 1]);
ensures: result >= 0
    && result <= map_len
    && forall(i, 0, result - 1, keys[i] <= keys[i + 1]);
body: {
    smap::sorted_map_remove(map_len, key)
};

atom test_sorted_map_len_after_insert(map_len: i64, map_cap: i64)
requires: map_len >= 0 && map_cap > 0 && map_len < map_cap;
ensures: result == map_len + 1 && result <= map_cap;
body: {
    smap::sorted_map_len_after_insert(map_len, map_cap)
};

atom test_sorted_map_remaining_capacity(map_len: i64, map_cap: i64)
requires: map_len >= 0 && map_cap > 0 && map_len <= map_cap;
ensures: result >= 0 && result == map_cap - map_len;
body: {
    smap::sorted_map_remaining_capacity(map_len, map_cap)
};

atom test_sorted_map_insert_position(map_len: i64, key: i64)
requires: map_len >= 0
    && forall(i, 0, map_len, keys[i] <= key || keys[i] > key);
ensures: result >= 0 && result <= map_len;
body: {
    smap::sorted_map_insert_position(map_len, key)
};

atom test_sorted_map_is_sorted(n: i64)
requires: n >= 0
    && forall(i, 0, n - 1, keys[i] <= keys[i + 1]);
ensures: result == 1;
body: {
    smap::sorted_map_is_sorted(n)
};
