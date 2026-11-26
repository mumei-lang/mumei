enum SelfList {
    Nil,
    Cons(value: u32, tail: SelfList),
}

enum Tree {
    Leaf(value: u32),
    Node(forest: Forest),
}

enum Forest {
    Nil,
    Cons(tree: Tree, rest: Forest, value: u32),
}

atom valid_self_recursive_write(l: SelfList, value: i64) -> bool
    requires: l == SelfList::Cons(1, SelfList::Nil) && value == 9;
    ensures: result == true;
    body: {
        l.value = value;
        true
    }

atom valid_mutual_recursive_write(forest: Forest, value: i64) -> bool
    requires: forest == Forest::Cons(Tree::Leaf(1), Forest::Nil, 1) && value == 9;
    ensures: result == true;
    body: {
        forest.value = value;
        true
    }
