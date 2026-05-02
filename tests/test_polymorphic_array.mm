// Polymorphic array type verification tests.

atom test_i64_array(n: i64)
requires: n >= 0 && forall(i, 0, n, arr[i] >= 0);
ensures: result == n;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        let x = arr[i];
        i = i + 1
    };
    n
};

atom test_f64_array(arr: [f64], n: i64) -> f64
requires: n >= 1 && forall(i, 0, n, arr[i] >= 0.0);
ensures: result == arr[0] && result >= 0.0;
body: arr[0];

atom test_bool_array(arr: [bool], n: i64) -> bool
requires: n >= 1 && forall(i, 0, n, arr[i] == true);
ensures: result == true;
body: arr[0];

atom test_array_element_type_inference(arr: [i64], n: i64)
requires: n >= 1 && forall(i, 0, n, arr[i] >= 0);
ensures: result >= 0;
body: {
    let x = arr[0];
    x
};
