enum Color {
    Red,
    Blue,
}

enum Tree {
    Leaf(color: Color),
    Node(next: Self),
}

struct Pt {
    x: i64,
    y: i64,
}

enum L {
    Nil,
    Cons(p: Pt, t: L),
}

struct BoundedPoint {
    value: u32,
}

enum U32List {
    Nil,
    Cons(p: BoundedPoint, t: U32List),
}

struct Buffer {
    xs: [i64],
}

enum ArrayList {
    End,
    More(payload: Buffer, tail: ArrayList),
}

atom nested_enum_payload_compares_to_constructor() -> bool
    requires: true;
    ensures: result == true;
    body: Tree::Node(Tree::Leaf(Color::Red)) != Tree::Leaf(Color::Blue)

atom struct_payload_match_binding() -> i64
    requires: true;
    ensures: result == 3;
    body: {
        let l = L::Cons(Pt { x: 3, y: 4 }, L::Nil);
        match l {
            L::Nil => 0,
            L::Cons(p, _) => p.x
        }
    }

atom struct_payload_tag_gated_read(l: L) -> i64
    requires: match l {
        L::Cons(p, _) => p.x == 8,
        _ => false
    };
    ensures: result == 8;
    body: l.p.x

atom struct_payload_leaf_write_is_exact(l: L, new_value: i64) -> bool
    requires: l == L::Cons(Pt { x: 3, y: 4 }, L::Nil) && new_value == 9;
    ensures: result == true;
    body: {
        l.p.x = new_value;
        l.p.x == new_value
    }

atom struct_payload_u32_leaf_is_bounded(l: U32List) -> bool
    requires: true;
    ensures: result == true;
    body: {
        match l {
            U32List::Nil => true,
            U32List::Cons(p, _) => p.value >= 0 && p.value <= 4294967295
        }
    }

atom recursive_function_with_struct_payload(l: L) -> i64
    requires: true;
    ensures: result == 0;
    body: {
        match l {
            L::Nil => 0,
            L::Cons(_, tail) => recursive_function_with_struct_payload(tail)
        }
    }

atom struct_array_payload_length_is_exact(l: ArrayList) -> i64
    requires: match l {
        ArrayList::More(payload, _) => len(payload.xs) == 4,
        _ => false
    };
    ensures: result == 4;
    body: len(l.payload.xs)
