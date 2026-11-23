enum Tree {
    Leaf(value: u32),
    Node(forest: Forest),
}

enum Forest {
    Nil,
    Cons(tree: Tree, rest: Forest, value: u32),
}

atom invalid_mutual_recursive_write(forest: Forest, value: i64) -> bool
    requires: forest == Forest::Cons(Tree::Leaf(1), Forest::Nil, 1)
        && value == 4294967296;
    ensures: result == true;
    body: {
        forest.value = value;
        true
    }
