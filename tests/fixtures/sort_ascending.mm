// Lean escalation fixture: insertion sort ascending-preservation.
// This atom intentionally includes `forall(i, 0, result - 1, arr[i] <= arr[i+1])`
// in ensures, which Z3 cannot discharge (spurious counterexample on Array + forall
// quantifier). Use `mumei verify --proof-cert --escalate-lean` to generate a
// proof certificate with `escalation_reason == "spurious_candidate"`, then pass
// to the mumei-lean bridge for Lean 4 discharge.
//
// See also: std/list.mm comments, MumeiLean.Sort.insertion_sort_ascending_bridge

atom verified_insertion_sort_ascending(n: i64)
requires: n >= 0 && forall(i, 0, n, arr[i] >= 0);
ensures: result == n && forall(i, 0, result - 1, arr[i] <= arr[i + 1]);
body: {
    if n <= 1 { n }
    else {
        let i = 1;
        while i < n
        invariant: i >= 1 && i <= n
        decreases: n - i
        {
            let key = arr[i];
            let j = i;
            while j > 0
            invariant: j >= 0 && j <= i
            decreases: j
            {
                if arr[j - 1] > key {
                    arr[j] = arr[j - 1];
                    j = j - 1
                } else {
                    j = 0
                }
            };
            arr[j] = key;
            i = i + 1
        };
        n
    }
};
