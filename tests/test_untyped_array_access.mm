// Fixtures for the `untyped_array_access` diagnostic (Item 3).
//
// `uses_untyped_array` accesses `arr[i]` without any `[T]` element-type
// annotation for `arr` (there is no `arr` parameter at all), so the element
// sort silently falls back to `i64`. This must produce an
// `untyped_array_access` diagnostic under `--warn-untyped-arrays` /
// `--strict-array-types`, while keeping the default `i64` fallback behavior.
//
// The remaining atoms annotate the array element type explicitly
// (`[i64]`/`[f64]`/`[bool]`) and must NOT trigger the diagnostic.

atom uses_untyped_array(n: i64) -> i64
requires: n >= 1 && forall(i, 0, n, arr[i] >= 0);
ensures: result >= 0;
body: arr[0];

atom uses_typed_i64_array(arr: [i64], n: i64) -> i64
requires: n >= 1 && forall(i, 0, n, arr[i] >= 0);
ensures: result >= 0;
body: arr[0];

atom uses_typed_f64_array(arr: [f64], n: i64) -> f64
requires: n >= 1 && forall(i, 0, n, arr[i] >= 0.0);
ensures: result >= 0.0;
body: arr[0];

atom uses_typed_bool_array(arr: [bool], n: i64) -> bool
requires: n >= 1 && forall(i, 0, n, arr[i] == true);
ensures: result == true;
body: arr[0];
