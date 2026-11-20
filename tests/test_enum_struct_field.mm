// Enum-typed struct fields `struct S { e: E }` (`pure-i64-composite/v17`).
// An enum field flattens into the same tag + payload leaves an enum
// parameter carries: `b.e` reads the whole enum value (consumable by
// `match`, equality, and pass-through), `b.e = e2` rewrites the whole
// enum slot, and struct literals seed the field from an enum
// constructor.

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

// A `match` on an enum field sees the declared variant patterns; the
// payload bindings carry the slot values stored under the field.
atom pick(b: Box) -> i64
requires: b.id >= 0;
ensures: result >= 0;
body: {
    let t = match b.c {
        Color::Red => 0,
        Color::Green(v) => v,
        Color::Blue(v, w) => v + w
    };
    if t >= 0 { b.id + t } else { b.id }
};

// A whole-enum write `b.c = <ctor>` replaces the field's tag and
// payload slots — a later `match` proves the written tag.
atom repaint(b: Box) -> i64
requires: b.id >= 0;
ensures: result >= 1;
body: {
    b.c = Color::Red;
    match b.c {
        Color::Red => b.id + 1,
        _ => 0
    }
};

// The same write with a bound enum value — `b.c = e2` rebinds the
// field to the value `e2` already holds.
atom repaint_bound(b: Box, e2: Color) -> i64
requires: b.id >= 0;
ensures: result >= 0;
body: {
    b.c = e2;
    match b.c {
        Color::Red => 1,
        Color::Green(v) => if v >= 0 { v } else { 0 },
        Color::Blue(v, w) => if v + w >= 0 { v + w } else { 0 }
    }
};

// A struct literal seeds the enum field from a constructor: the
// payload slot reads back as the constructor's argument.
atom wrap(v: i64) -> i64
requires: v >= 0;
ensures: result >= 0;
body: {
    let b = Box { id: v, c: Color::Green(v) };
    match b.c {
        Color::Green(x) => x,
        _ => 0
    }
};

// An enum field passes through a call: the callee sees the same tag
// and payload slots the field holds.
atom hue(c: Color) -> i64
requires: true;
ensures: result >= 0;
body: {
    match c {
        Color::Red => 1,
        Color::Green(v) => if v >= 0 { v } else { 0 },
        Color::Blue(v, w) => if v + w >= 0 { v + w } else { 0 }
    }
};

atom relay(b: Box) -> i64
requires: true;
ensures: result >= 0;
body: {
    hue(b.c)
};

// A struct result keeps its enum field: the caller can match on the
// returned composite's field.
atom boxed(c: Color) -> Box
requires: true;
ensures: result.id == 0;
body: {
    Box { id: 0, c: c }
};

atom boxed_hue(c: Color) -> i64
requires: true;
ensures: result >= 0;
body: {
    let b = boxed(c);
    hue(b.c)
};

// A conditional whole-enum write merges per-branch enum values —
// either arm leaves `b.c` well-formed for the following match.
atom repaint_branch(b: Box, e2: Color) -> i64
requires: true;
ensures: result >= 0;
body: {
    if b.id > 0 {
        b.c = e2;
    } else {
        b.c = Color::Red;
    }
    match b.c {
        Color::Red => 1,
        Color::Green(v) => if v >= 0 { v } else { 0 },
        Color::Blue(v, w) => if v + w >= 0 { v + w } else { 0 }
    }
};

// Enum fields nest inside struct-typed fields too — `o.inner.c`
// reads the enum carried by the nested struct.
struct Holder {
    inner: Box,
    tag: i64
}

atom nested_pick(o: Holder) -> i64
requires: true;
ensures: result >= 0;
body: {
    match o.inner.c {
        Color::Red => 1,
        Color::Green(v) => if v >= 0 { v } else { 0 },
        Color::Blue(v, w) => if v + w >= 0 { v + w } else { 0 }
    }
};

// A `requires` pinning the pre-write variant must not turn into a
// contradiction once `b.c = …` rebinding is in the body — the
// requires-time view of a written enum field is havoc'd like a
// scalar write (review fix for `pure-i64-composite/v17`).
atom prepost(b: Box) -> i64
requires: b.c == Color::Red;
ensures: result == 1;
body: {
    b.c = Color::Green(1);
    1
};
