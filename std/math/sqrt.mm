// =============================================================
// std/math/sqrt — verified integer square root
// =============================================================
// Binary search for the largest r with r * r <= n.

atom isqrt(n: i64)
    requires: n >= 0;
    ensures: result >= 0 && result * result <= n && n < (result + 1) * (result + 1);
    body: {
        if n < 1 { 0 } else {
            let lo = 1;
            let hi = n;
            let mid = 0;
            while lo < hi
            invariant: lo >= 1 && lo <= hi && hi <= n && lo * lo <= n && n < (hi + 1) * (hi + 1)
            decreases: hi - lo
            {
                mid = (lo + hi + 1) / 2;
                if mid * mid <= n { lo = mid } else { hi = mid - 1 }
            };
            lo
        }
    };
