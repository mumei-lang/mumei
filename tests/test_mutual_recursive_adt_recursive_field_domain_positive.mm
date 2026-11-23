enum Tree {
    Leaf(value: u32),
    Node(forest: Forest),
}

enum Forest {
    Nil,
    Cons(tree: Tree, rest: Forest, value: u32),
}

atom rewrap_recursive_field_read(tree: Tree) -> Tree
    requires: true;
    ensures: result == tree;
    body: match tree {
        Tree::Leaf(value) => Tree::Leaf(value),
        Tree::Node(_) => Tree::Node(tree.forest)
    }

atom rewrap_recursive_field_pattern(tree: Tree) -> Tree
    requires: true;
    ensures: result == tree;
    body: match tree {
        Tree::Leaf(value) => Tree::Leaf(value),
        Tree::Node(forest) => Tree::Node(forest)
    }

atom rewrap_cross_member_field_pattern(forest: Forest) -> Forest
    requires: true;
    ensures: result == forest;
    body: match forest {
        Forest::Nil => Forest::Nil,
        Forest::Cons(tree, rest, value) => Forest::Cons(tree, rest, value)
    }
