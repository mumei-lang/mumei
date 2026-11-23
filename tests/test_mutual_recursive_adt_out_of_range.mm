enum Tree {
    Leaf(value: u32),
    Node(forest: Forest),
}

enum Forest {
    Nil,
    Cons(tree: Tree, rest: Forest),
}

atom out_of_range_u32_constructor() -> Tree
    requires: true;
    ensures: true;
    body: Tree::Leaf(4294967296)
