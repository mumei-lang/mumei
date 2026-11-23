enum Tree {
    Leaf(value: u32),
    Node(forest: Forest),
}

enum Forest {
    Nil,
    Cons(tree: Tree, rest: Forest),
}

atom construct_and_match_across_members() -> bool
    requires: true;
    ensures: result == true;
    body: {
        let tree = Tree::Node(Forest::Cons(Tree::Leaf(7), Forest::Nil));
        match tree {
            Tree::Leaf(_) => false,
            Tree::Node(forest) => match forest {
                Forest::Nil => false,
                Forest::Cons(tree, _) => match tree {
                    Tree::Leaf(value) => value == 7,
                    Tree::Node(_) => false
                }
            }
        }
    }

atom nested_cross_member_match() -> bool
    requires: true;
    ensures: result == true;
    body: match Tree::Node(Forest::Cons(Tree::Leaf(7), Forest::Nil)) {
        Tree::Node(Forest::Cons(Tree::Leaf(value), _)) => value == 7,
        _ => false
    }

atom mutual_enum_equality() -> bool
    requires: true;
    ensures: result == true;
    body: Tree::Node(Forest::Cons(Tree::Leaf(7), Forest::Nil))
        == Tree::Node(Forest::Cons(Tree::Leaf(7), Forest::Nil))

atom u32_payload_domain_in_tree(tree: Tree) -> bool
    requires: true;
    ensures: result == true;
    body: match tree {
        Tree::Leaf(value) => value >= 0 && value <= 4294967295,
        Tree::Node(_) => true
    }

atom tree_walk(tree: Tree, depth: i64) -> i64
    requires: depth >= 0;
    decreases: depth;
    ensures: result >= 0;
    body: if depth == 0 {
        0
    } else {
        match tree {
            Tree::Leaf(_) => 1,
            Tree::Node(forest) => forest_walk(forest, depth - 1)
        }
    }

atom forest_walk(forest: Forest, depth: i64) -> i64
    requires: depth >= 0;
    decreases: depth;
    ensures: result >= 0;
    body: if depth == 0 {
        0
    } else {
        match forest {
            Forest::Nil => 0,
            Forest::Cons(tree, _) => tree_walk(tree, depth - 1)
        }
    }
