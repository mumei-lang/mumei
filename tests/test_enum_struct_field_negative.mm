// Enum-typed struct field rejections (`pure-i64-composite/v17`).
// Every atom here must fail: payload/tag paths through the field are
// not assignable (payload writes live in the v16 slot-write profile
// on enum bindings, not through a struct field), a whole-field write
// needs the same declared enum, a tail-position write cannot yield a
// scalar, and a wrong postcondition over the written tag is caught.

enum Color {
    Red,
    Green(i64),
    Blue(i64, i64)
}

enum Size {
    Small,
    Large
}

struct Box {
    id: i64,
    c: Color
}

// `b.c.v` names no leaf: a payload write through the struct field is
// not admitted — v16 payload writes target enum bindings directly.
atom bad_payload_write(b: Box) -> i64
requires: true;
ensures: result == 1;
body: {
    b.c.v = 1;
    b.id
};

// `b.c.tag` is not a declared field either — the tag is only set by
// constructors and whole-enum writes.
atom bad_tag_write(b: Box) -> i64
requires: true;
ensures: result == 1;
body: {
    b.c.tag = 1;
    b.id
};

// A whole-field write requires the same declared enum — `Size` is
// not `Color`.
atom bad_type_mismatch(b: Box) -> i64
requires: true;
ensures: result == 1;
body: {
    b.c = Size::Small;
    b.id
};

// A scalar cannot seat an enum-typed field.
atom bad_scalar_write(b: Box) -> i64
requires: true;
ensures: result == 1;
body: {
    b.c = 3;
    b.id
};

// A field write in tail position cannot yield a scalar kind.
atom bad_tail_write(b: Box) -> i64
requires: true;
ensures: result == 1;
body: {
    b.c = Color::Red
};

// After `b.c = Color::Red` the tag is provably Red — claiming the
// Green arm fires is a wrong postcondition.
atom bad_wrong_tag(b: Box) -> i64
requires: true;
ensures: result == 0;
body: {
    b.c = Color::Red;
    match b.c {
        Color::Red => 1,
        _ => 0
    }
};
