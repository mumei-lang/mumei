// =============================================================
// Negative: sorted-map invariant violation
// =============================================================
// Stores a smaller key after a larger key while claiming nondecreasing order.

atom sorted_map_invariant_violation()
requires: len(keys) >= 2;
ensures: forall(i, 0, 1, keys[i] <= keys[i + 1]);
body: {
    keys[0] = 10;
    keys[1] = 1;
    2
};
