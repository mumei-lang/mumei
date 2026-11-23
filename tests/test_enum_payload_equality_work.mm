enum Leaf {
    N,
    D(value: i64)
}

enum Level1 {
    N,
    D(a: Leaf, b: Leaf, c: Leaf, d: Leaf, e: Leaf)
}

enum Level2 {
    N,
    D(a: Level1, b: Level1, c: Level1, d: Level1, e: Level1)
}

enum Level3 {
    N,
    D(a: Level2, b: Level2, c: Level2, d: Level2, e: Level2)
}

enum Level4 {
    N,
    D(a: Level3, b: Level3, c: Level3, d: Level3, e: Level3)
}

enum Level5 {
    N,
    D(a: Level4, b: Level4, c: Level4, d: Level4, e: Level4)
}

enum Level6 {
    N,
    D(a: Level5, b: Level5, c: Level5, d: Level5, e: Level5)
}

enum Level7 {
    N,
    D(a: Level6, b: Level6, c: Level6, d: Level6, e: Level6)
}

enum Level8 {
    N,
    D(a: Level7, b: Level7, c: Level7, d: Level7, e: Level7)
}

atom array_free_nested_enum_equality_finishes_quickly(value: Level8) -> bool
requires: true;
ensures: result == true;
body: value == value;
