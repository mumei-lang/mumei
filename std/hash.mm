// =============================================================
// std/hash — verified hashable primitives
// =============================================================
// バケット計算と非負 hash witness を検証する。

atom hash_bucket(h: i64, num_buckets: i64)
requires: h >= 0 && num_buckets > 0;
ensures: result >= 0 && result < num_buckets;
body: {
    h - (h / num_buckets) * num_buckets
};

atom hash_combine(h1: i64, h2: i64)
requires: h1 >= 0 && h1 <= 1000000 && h2 >= 0 && h2 <= 1000000;
ensures: result >= 0;
body: {
    h1 + h2
};

atom hash_i64_nonneg(x: i64, max_hash: i64)
requires: x >= 0 && max_hash > 0;
ensures: result >= 0 && result <= max_hash;
body: {
    if x <= max_hash { x } else { max_hash }
};

atom hash_eq_consistent(a: i64, b: i64)
requires: true;
ensures: result >= 0 && result <= 1;
body: {
    if a == b { 1 } else { 0 }
};
