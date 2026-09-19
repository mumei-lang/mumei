// P10-C: finite non-recursive enums lower to native Z3 Datatype sorts.
// Variants are constructors with is-<V> testers and per-field selectors
// carrying the payload's real sort (Str → String, f64 → Real/Float, …).
//
// Usage: mumei verify tests/test_datatype_enum.mm

enum Color {
    Red,
    Green,
    Blue,
}

enum Shape {
    Point,
    Named(Str),
}

enum Metric {
    Rate(f64),
    Count(i64),
    Enabled(bool),
}

// enum-typed parameter: match via is-<Variant> testers; exhaustiveness is
// decided by the datatype's completeness axiom (no Int domain constraint).
atom code_of(c: Color) -> i64
    requires: true;
    ensures: result >= 0;
    body: {
        match c {
            Red => 0,
            Green => 1,
            Blue => 2
        }
    }

// Str payload: the selector yields a Z3 String, so `n == "x"` is a real
// string comparison (under Int-tag encoding the projector was an
// unconstrained Int and this clause could not be modelled).
atom shape_is_x(s: Shape) -> bool
    requires: true;
    ensures: true;
    body: {
        match s {
            Point => false,
            Named(n) => n == "x"
        }
    }

// requires-side datatype equality prunes the Named arm entirely.
atom only_points(s: Shape) -> i64
    requires: s == Shape::Point;
    ensures: result == 0;
    body: {
        match s {
            Point => 0,
            Named(n) => 1
        }
    }

// Constructor injectivity: Point and Named("x") are provably distinct even
// though both lower through the same enum sort.
atom ctor_distinct() -> bool
    requires: true;
    ensures: result == true;
    body: {
        Shape::Point != Shape::Named("x")
    }
}

// Payload equality flows through the selector: s == Named("hi") forces the
// Named_0(s) projection to "hi", which the match arm returns.
atom named_roundtrip(s: Shape) -> Str
    requires: s == Shape::Named("hi");
    ensures: result == "hi";
    body: {
        match s {
            Point => "origin",
            Named(n) => n
        }
    }

// f64 payload selectors produce Real; numeric comparison works on them.
atom metric_is_positive(m: Metric) -> bool
    requires: true;
    ensures: true;
    body: {
        match m {
            Rate(r) => r > 0.0,
            Count(n) => n > 0,
            Enabled(b) => false
        }
    }
}

enum IntList {
    Nil,
    Cons(i64, IntList),
}

// Recursive ADT keeps the Int-tag encoding and the `inductive_data_type`
// fragment tag (Lean boundary), yet still verifies when Z3 decides it.
atom head_or(l: IntList) -> i64
    requires: true;
    ensures: true;
    body: {
        match l {
            Nil => 0,
            Cons(h, t) => h
        }
    }
}
