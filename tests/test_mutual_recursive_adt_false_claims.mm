enum Tree {
    Leaf(value: u32),
    Node(forest: Forest),
}

enum Forest {
    Nil,
    Cons(tree: Tree, rest: Forest),
}

atom stale_value_claim() -> bool
    requires: true;
    ensures: result == true;
    body: {
        let old_tree = Tree::Leaf(3);
        let new_tree = Tree::Leaf(4);
        match old_tree {
            Tree::Leaf(old_value) => match new_tree {
                Tree::Leaf(new_value) => new_value == old_value,
                Tree::Node(_) => true
            },
            Tree::Node(_) => true
        }
    }

atom wrong_variant_claim(tree: Tree) -> bool
    requires: tree == Tree::Leaf(3);
    ensures: result == true;
    body: match tree {
        Tree::Node(_) => true,
        Tree::Leaf(_) => false
    }
