atom recursive_array_result(n: i64) -> [i64]
    requires: true;
    ensures: true;
    decreases: n;
    body: {
        if n > 0 {
            let direct = recursive_array_result(n - 1);
            let referenced = call(atom_ref(recursive_array_result), n - 1);
            direct
        } else {
            [0]
        }
    }

atom recursive_result_lengths_equal(n: i64) -> bool
    requires: n >= 0;
    ensures: result == true;
    body: {
        let direct = recursive_array_result(n);
        let referenced = call(atom_ref(recursive_array_result), n);
        len(direct) == len(referenced)
    }
