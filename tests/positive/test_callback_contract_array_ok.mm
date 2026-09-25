atom arr_len(a: [i64]) -> i64
    requires: true;
    ensures: result == len(a);
    body: len(a);

atom apply_array(a: [i64], f: atom_ref([i64]) -> i64) -> i64
    requires: true;
    ensures: result >= 0;
    contract(f): ensures: result >= 0;
    body: call(f, a);

atom use_array_callback(a: [i64]) -> i64
    requires: true;
    ensures: result >= 0;
    body: apply_array(a, atom_ref(arr_len));
