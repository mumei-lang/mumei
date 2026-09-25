atom string_length(s: Str) -> i64
    requires: true;
    ensures: result == len(s);
    body: len(s);

atom apply_nonneg(s: Str, f: atom_ref(Str) -> i64) -> i64
    requires: true;
    ensures: result >= 0;
    contract(f): ensures: result >= 0;
    body: call(f, s);

atom use_string_callback(s: Str) -> i64
    requires: true;
    ensures: result >= 0;
    body: apply_nonneg(s, atom_ref(string_length));
