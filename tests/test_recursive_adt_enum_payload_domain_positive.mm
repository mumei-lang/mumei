struct B { value: u32 }
enum L { Nil, Cons(p: B, t: L) }
enum F { None, Some(v: u32) }
enum Inner { Value(v: u32) }
enum Outer { Wrap(inner: Inner) }
enum ArrayPayload { Empty, Values(values: [u32]) }

atom enum_payload_u32_literals_are_in_range() -> bool
    requires: true;
    ensures: result == true;
    body: {
        let rec = L::Cons(B { value: 42 }, L::Nil);
        let finite = F::Some(7);
        let outer = Outer::Wrap(Inner::Value(11));
        let rec_ok = match rec {
            L::Nil => false,
            L::Cons(p, _) => p.value == 42
        };
        let finite_ok = match finite {
            F::None => false,
            F::Some(v) => v == 7
        };
        let nested_ok = match outer {
            Outer::Wrap(inner) => match inner {
                Inner::Value(v) => v == 11
            }
        };
        rec_ok && finite_ok && nested_ok
    }

atom enum_payload_u32_parameter_passes(n: u32) -> bool
    requires: true;
    ensures: result == true;
    body: {
        let finite = F::Some(n);
        match finite {
            F::None => false,
            F::Some(v) => v == n
        }
    }

atom enum_payload_bounded_i64_passes(n: i64) -> bool
    requires: n >= 0 && n <= 4294967295;
    ensures: result == true;
    body: {
        let finite = F::Some(n);
        match finite {
            F::None => false,
            F::Some(v) => v == n
        }
    }

atom enum_payload_u32_array_literal_passes() -> bool
    requires: true;
    ensures: result == true;
    body: {
        let payload = ArrayPayload::Values([7, 11]);
        match payload {
            ArrayPayload::Empty => false,
            ArrayPayload::Values(values) =>
                len(values) == 2 && values[0] == 7 && values[1] == 11
        }
    }

atom enum_payload_u32_array_parameter_passes(values: [u32]) -> bool
    requires: len(values) > 0;
    ensures: result == true;
    body: {
        let payload = ArrayPayload::Values(values);
        match payload {
            ArrayPayload::Empty => false,
            ArrayPayload::Values(items) => {
                let first = items[0];
                first == first
            }
        }
    }
